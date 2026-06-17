//! Gemini → Kiro 请求转换器

use std::collections::HashMap;
use uuid::Uuid;

use crate::anthropic::converter::map_model;
use crate::kiro::model::requests::conversation::{
    ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, Message, UserInputMessage, UserMessage,
};
use crate::kiro::model::requests::kiro::KiroRequest;

use super::types::GenerateContentRequest;

#[allow(dead_code)]
pub struct ConversionResult {
    pub kiro_request: KiroRequest,
    pub model: String,
    pub tool_name_map: HashMap<String, String>,
}

pub fn convert_gemini_to_kiro(
    req: &GenerateContentRequest,
    model_from_path: &str,
    profile_arn: Option<String>,
) -> Result<ConversionResult, String> {
    let model_id = map_model(model_from_path).unwrap_or_else(|| {
        tracing::warn!("未知 Gemini 模型 '{}', 回退到默认", model_from_path);
        "claude-sonnet-4.5".to_string()
    });

    let mut system_prompt = String::new();
    if let Some(instruction) = &req.system_instruction {
        for part in &instruction.parts {
            if let Some(text) = &part.text {
                if !system_prompt.is_empty() {
                    system_prompt.push('\n');
                }
                system_prompt.push_str(text);
            }
        }
    }

    let mut history: Vec<Message> = Vec::new();
    let mut current_content = String::new();

    if req.contents.is_empty() {
        return Err("contents 列表为空".to_string());
    }

    for (i, content) in req.contents.iter().enumerate() {
        let is_last = i == req.contents.len() - 1;
        let role = content.role.as_deref().unwrap_or("user");
        let text: String = content
            .parts
            .iter()
            .filter_map(|p| p.text.as_deref())
            .collect::<Vec<_>>()
            .join("");

        match role {
            "user" => {
                if is_last {
                    current_content = text;
                } else {
                    history.push(Message::User(HistoryUserMessage {
                        user_input_message: UserMessage::new(&text, &model_id),
                    }));
                }
            }
            "model" => {
                let content_text = if text.trim().is_empty() {
                    "I understand.".to_string()
                } else {
                    text
                };
                history.push(Message::Assistant(HistoryAssistantMessage::new(
                    &content_text,
                )));
            }
            _ => {}
        }
    }

    if current_content.is_empty() {
        current_content = "Continue.".to_string();
    }

    if !system_prompt.is_empty() {
        let system_messages = vec![
            Message::User(HistoryUserMessage {
                user_input_message: UserMessage::new(&system_prompt, &model_id),
            }),
            Message::Assistant(HistoryAssistantMessage::new(
                "I will follow these instructions.",
            )),
        ];
        history.splice(0..0, system_messages);
    }

    let user_input = UserInputMessage::new(&current_content, &model_id);

    let conversation_id = Uuid::new_v4().to_string();
    let conversation_state = ConversationState::new(&conversation_id)
        .with_chat_trigger_type("MANUAL")
        .with_current_message(CurrentMessage::new(user_input))
        .with_history(history);

    let kiro_request = KiroRequest {
        conversation_state,
        profile_arn,
        additional_model_request_fields: None,
    };

    Ok(ConversionResult {
        kiro_request,
        model: model_from_path.to_string(),
        tool_name_map: HashMap::new(),
    })
}
