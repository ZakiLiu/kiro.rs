//! OpenAI → Kiro 请求转换器

use std::collections::HashMap;

use uuid::Uuid;

use crate::anthropic::converter::{
    build_additional_model_request_fields, map_model, thinking_config_for_model,
};
use crate::anthropic::types::{MessagesRequest, OutputConfig, Thinking};
use crate::kiro::model::requests::conversation::KiroImage;
use crate::kiro::model::requests::conversation::{
    AssistantMessage, ConversationState, CurrentMessage, HistoryAssistantMessage,
    HistoryUserMessage, Message, UserInputMessage, UserInputMessageContext, UserMessage,
};
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::model::requests::tool::{
    InputSchema, Tool, ToolResult, ToolSpecification, ToolUseEntry,
};

use super::types::{ChatCompletionRequest, ChatMessage, MessageContent};

#[allow(dead_code)]
pub struct ConversionResult {
    pub kiro_request: KiroRequest,
    pub model: String,
    pub tool_name_map: HashMap<String, String>,
}

pub fn convert_openai_to_kiro(
    req: &ChatCompletionRequest,
    profile_arn: Option<String>,
) -> Result<ConversionResult, String> {
    let model_id = resolve_openai_model_id(&req.model);

    let mut system_prompt = String::new();
    let mut non_system_messages: Vec<&ChatMessage> = Vec::new();

    for msg in &req.messages {
        if msg.role == "system" {
            if let Some(content) = &msg.content {
                let text = content_to_text(content);
                if !text.is_empty() {
                    if !system_prompt.is_empty() {
                        system_prompt.push('\n');
                    }
                    system_prompt.push_str(&text);
                }
            }
        } else {
            non_system_messages.push(msg);
        }
    }

    if non_system_messages.is_empty() {
        return Err("消息列表为空（去除 system 消息后）".to_string());
    }

    let mut history: Vec<Message> = Vec::new();
    let mut tool_results: Vec<ToolResult> = Vec::new();
    let mut current_content = String::new();
    let mut images: Vec<KiroImage> = Vec::new();
    let tool_name_map = HashMap::new();

    for (i, msg) in non_system_messages.iter().enumerate() {
        let is_last = i == non_system_messages.len() - 1;

        match msg.role.as_str() {
            "user" => {
                let (text, msg_images) = extract_user_content(msg);
                if is_last {
                    current_content = text;
                    images = msg_images;
                } else {
                    let mut user_msg = UserMessage::new(&text, &model_id);
                    if !msg_images.is_empty() {
                        user_msg = user_msg.with_images(msg_images);
                    }
                    history.push(Message::User(HistoryUserMessage {
                        user_input_message: user_msg,
                    }));
                }
            }
            "assistant" => {
                let text = msg
                    .content
                    .as_ref()
                    .map(content_to_text)
                    .unwrap_or_default();
                let content = if text.trim().is_empty() && msg.tool_calls.is_some() {
                    " ".to_string()
                } else if text.trim().is_empty() {
                    "I understand.".to_string()
                } else {
                    text
                };

                let mut assistant_msg = AssistantMessage::new(&content);

                if let Some(tool_calls) = &msg.tool_calls {
                    let tool_uses: Vec<ToolUseEntry> = tool_calls
                        .iter()
                        .filter(|tc| tc.call_type == "function")
                        .map(|tc| {
                            let input: serde_json::Value =
                                serde_json::from_str(&tc.function.arguments)
                                    .unwrap_or(serde_json::json!({}));
                            ToolUseEntry::new(&tc.id, &tc.function.name).with_input(input)
                        })
                        .collect();
                    if !tool_uses.is_empty() {
                        assistant_msg = assistant_msg.with_tool_uses(tool_uses);
                    }
                }

                history.push(Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant_msg,
                }));
            }
            "tool" => {
                if let Some(tool_call_id) = &msg.tool_call_id {
                    let text = msg
                        .content
                        .as_ref()
                        .map(content_to_text)
                        .unwrap_or_else(|| "(no output)".to_string());
                    tool_results.push(ToolResult::success(tool_call_id, &text));

                    let next_msg = non_system_messages.get(i + 1);
                    let should_flush = next_msg.map(|m| m.role != "tool").unwrap_or(true);

                    if should_flush && !tool_results.is_empty() && !is_last {
                        let mut ctx = UserInputMessageContext::new();
                        ctx = ctx.with_tool_results(std::mem::take(&mut tool_results));
                        let user_msg =
                            UserMessage::new("Tool results provided.", &model_id).with_context(ctx);
                        history.push(Message::User(HistoryUserMessage {
                            user_input_message: user_msg,
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    // 如果最后一条是 assistant，自动续
    if !history.is_empty()
        && let Some(Message::Assistant(_)) = history.last()
        && current_content.is_empty()
    {
        current_content = "Continue.".to_string();
    }

    // 如果没有 current_content 但有 tool_results
    if current_content.is_empty() && !tool_results.is_empty() {
        current_content = "Tool results provided.".to_string();
    }

    let final_content = if current_content.is_empty() {
        "Continue.".to_string()
    } else {
        current_content
    };

    // System prompt 注入到 history 头部（与 Kiro IDE 一致）
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

    // 转换工具
    let tools = convert_openai_tools(&req.tools);

    // 构建 context
    let mut ctx = UserInputMessageContext::new();
    if !tools.is_empty() {
        ctx = ctx.with_tools(tools);
    }
    if !tool_results.is_empty() {
        ctx = ctx.with_tool_results(tool_results);
    }

    let mut user_input = UserInputMessage::new(&final_content, &model_id)
        .with_context(ctx)
        .with_origin("AI_EDITOR");
    if !images.is_empty() {
        user_input = user_input.with_images(images);
    }

    let conversation_id = Uuid::new_v4().to_string();
    let agent_continuation_id =
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, conversation_id.as_bytes()).to_string();

    let conversation_state = ConversationState::new(&conversation_id)
        .with_agent_continuation_id(agent_continuation_id)
        .with_agent_task_type("vibe")
        .with_chat_trigger_type("MANUAL")
        .with_current_message(CurrentMessage::new(user_input))
        .with_history(history);

    // Thinking fields
    let additional_fields = build_openai_thinking_fields(req, &model_id);

    let kiro_request = KiroRequest {
        conversation_state,
        profile_arn,
        additional_model_request_fields: additional_fields,
    };

    Ok(ConversionResult {
        kiro_request,
        model: req.model.clone(),
        tool_name_map,
    })
}

fn resolve_openai_model_id(model: &str) -> String {
    let normalized = normalize_openai_model_for_kiro(model);

    // `auto-kiro` 是 kiro-gateway 的公开别名，用来避开部分 IDE 自己的 `auto`；
    // `auto` 本身虽然不在列表中展示，也应保持可直连 runtime。
    if normalized == "auto" {
        tracing::info!(
            requested_model = %model,
            normalized_model = %normalized,
            "OpenAI 模型别名按 Kiro runtime pass-through 处理"
        );
        return normalized;
    }

    if let Some(mapped) = map_model(model).or_else(|| map_model(&normalized)) {
        return mapped;
    }

    tracing::info!(
        requested_model = %model,
        normalized_model = %normalized,
        "未知 OpenAI 模型按 Kiro runtime pass-through 处理"
    );
    normalized
}

fn normalize_openai_model_for_kiro(model: &str) -> String {
    let mut normalized = model.trim().trim_matches('/').to_ascii_lowercase();
    if let Some(stripped) = normalized.strip_prefix("models/") {
        normalized = stripped.to_string();
    }
    normalized = normalized.split_whitespace().collect::<Vec<_>>().join("-");
    normalized = normalized.replace('_', "-");

    match normalized.as_str() {
        "auto-kiro" => return "auto".to_string(),
        "deepseek-v3.2" | "deepseek-v3-2" | "deepseek-3-2" => {
            return "deepseek-3.2".to_string();
        }
        "glm5" => return "glm-5".to_string(),
        "minimax-m2-1" => return "minimax-m2.1".to_string(),
        "minimax-m2-5" => return "minimax-m2.5".to_string(),
        "qwen-3-coder-next" => return "qwen3-coder-next".to_string(),
        _ => {}
    }

    if let Some(rest) = normalized.strip_prefix("deepseek-") {
        let rest = rest.strip_prefix('v').unwrap_or(rest);
        if rest == "3.2" || rest == "3-2" {
            return "deepseek-3.2".to_string();
        }
    }

    normalized
}

fn build_openai_thinking_fields(
    req: &ChatCompletionRequest,
    model_id: &str,
) -> Option<serde_json::Value> {
    // 从 reasoning_effort 直接构建
    if let Some(effort) = &req.reasoning_effort {
        let thinking_config = thinking_config_for_model(model_id);
        let fake_req = MessagesRequest {
            model: req.model.clone(),
            max_tokens: req.max_tokens.unwrap_or(8192),
            messages: vec![],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: Some(Thinking {
                thinking_type: "adaptive".to_string(),
                budget_tokens: 0,
            }),
            output_config: Some(OutputConfig {
                effort: effort.clone(),
            }),
            metadata: None,
            reasoning_effort: Some(effort.clone()),
        };
        return build_additional_model_request_fields(&fake_req, thinking_config.as_ref());
    }

    // 从 thinking 对象构建
    if let Some(thinking_val) = &req.thinking
        && let Some(obj) = thinking_val.as_object()
    {
        let t_type = obj
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("disabled");
        if t_type == "disabled" {
            return None;
        }
        let budget = obj
            .get("budget_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(20000) as i32;
        let thinking_config = thinking_config_for_model(model_id);
        let fake_req = MessagesRequest {
            model: req.model.clone(),
            max_tokens: req.max_tokens.unwrap_or(8192),
            messages: vec![],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: Some(Thinking {
                thinking_type: t_type.to_string(),
                budget_tokens: budget,
            }),
            output_config: None,
            metadata: None,
            reasoning_effort: None,
        };
        return build_additional_model_request_fields(&fake_req, thinking_config.as_ref());
    }

    None
}

fn content_to_text(content: &MessageContent) -> String {
    match content {
        MessageContent::Text(s) => s.clone(),
        MessageContent::Parts(parts) => parts
            .iter()
            .filter(|p| p.part_type == "text")
            .filter_map(|p| p.text.as_ref())
            .cloned()
            .collect::<Vec<_>>()
            .join(""),
    }
}

fn extract_user_content(msg: &ChatMessage) -> (String, Vec<KiroImage>) {
    let mut text = String::new();
    let mut images = Vec::new();

    match &msg.content {
        Some(MessageContent::Text(s)) => {
            text = s.clone();
        }
        Some(MessageContent::Parts(parts)) => {
            for part in parts {
                match part.part_type.as_str() {
                    "text" => {
                        if let Some(t) = &part.text {
                            text.push_str(t);
                        }
                    }
                    "image_url" => {
                        if let Some(img_url) = &part.image_url
                            && let Some(kiro_img) = parse_data_url_image(&img_url.url)
                        {
                            images.push(kiro_img);
                        }
                    }
                    _ => {}
                }
            }
        }
        None => {}
    }

    (text, images)
}

fn parse_data_url_image(url: &str) -> Option<KiroImage> {
    if !url.starts_with("data:image/") {
        return None;
    }
    let rest = url.strip_prefix("data:image/")?;
    let semi_pos = rest.find(';')?;
    let format = &rest[..semi_pos];
    let after_semi = &rest[semi_pos + 1..];
    let data = after_semi.strip_prefix("base64,")?;
    let normalized_format = match format {
        "jpg" | "jpeg" => "jpeg",
        "png" => "png",
        "gif" => "gif",
        "webp" => "webp",
        _ => return None,
    };
    Some(KiroImage::from_base64(normalized_format, data))
}

const TOOL_DESC_MAX_LEN: usize = 10237;

fn convert_openai_tools(tools: &Option<Vec<super::types::ChatTool>>) -> Vec<Tool> {
    let Some(tools) = tools else {
        return vec![];
    };

    tools
        .iter()
        .map(|tool| {
            let mut desc = tool.function.description.clone();
            if desc.is_empty() {
                desc = format!("Tool: {}", tool.function.name);
            }
            if desc.len() > TOOL_DESC_MAX_LEN {
                desc.truncate(TOOL_DESC_MAX_LEN);
                desc.push_str("...");
            }
            Tool {
                tool_specification: ToolSpecification {
                    name: tool.function.name.clone(),
                    description: desc,
                    input_schema: InputSchema {
                        json: tool
                            .function
                            .parameters
                            .clone()
                            .unwrap_or(serde_json::json!({"type": "object", "properties": {}})),
                    },
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_for_model(model: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some(MessageContent::Text("hello".to_string())),
                reasoning_content: None,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            stream: false,
            temperature: None,
            top_p: None,
            max_tokens: None,
            tools: None,
            tool_choice: None,
            reasoning_effort: None,
            thinking: None,
            metadata: None,
        }
    }

    fn converted_current_model_id(model: &str) -> String {
        convert_openai_to_kiro(&request_for_model(model), None)
            .unwrap()
            .kiro_request
            .conversation_state
            .current_message
            .user_input_message
            .model_id
    }

    #[test]
    fn unknown_runtime_models_pass_through_instead_of_falling_back() {
        assert_eq!(converted_current_model_id("deepseek-v3.2"), "deepseek-3.2");
        assert_eq!(
            converted_current_model_id("models/qwen3-coder-next"),
            "qwen3-coder-next"
        );
        assert_eq!(converted_current_model_id("MiniMax M2.5"), "minimax-m2.5");
        assert_eq!(converted_current_model_id("GLM 5"), "glm-5");
    }

    #[test]
    fn openai_auto_kiro_alias_passes_runtime_auto() {
        assert_eq!(converted_current_model_id("auto-kiro"), "auto");
        assert_eq!(converted_current_model_id("auto"), "auto");
    }

    #[test]
    fn known_openai_aliases_still_use_existing_model_mapping() {
        assert_eq!(converted_current_model_id("gpt-4"), "claude-sonnet-4.5");
        assert_eq!(
            converted_current_model_id("models/gpt-4"),
            "claude-sonnet-4.5"
        );
    }
}
