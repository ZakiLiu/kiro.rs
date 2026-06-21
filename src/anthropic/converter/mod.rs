//! Anthropic → Kiro 协议转换器
//!
//! 负责将 Anthropic API 请求格式转换为 Kiro API 请求格式

mod content;
mod history;
mod model;
mod schema;
mod system;
mod tools;

use std::collections::HashMap;

use uuid::Uuid;

use crate::kiro::model::requests::conversation::{
    ConversationState, CurrentMessage, UserInputMessage, UserInputMessageContext,
};

use crate::anthropic::types::ContentBlock;
use crate::model::config::CompressionConfig;

// Re-export public API
pub use model::{ConversionError, ConversionResult, is_agentic_model, map_model};

use content::{count_images_in_content, non_empty_content_or_space, process_message_content};
use history::{
    BuildHistoryContext, build_history, extract_session_id, messages_history_fingerprint,
    remove_orphaned_tool_uses, validate_tool_pairing,
};
use model::MAX_TOTAL_IMAGES;
use system::determine_chat_trigger_type;
pub(crate) use system::{
    build_additional_model_request_fields, output_config_thinking_schema, thinking_config_for_model,
};
use tools::{collect_history_tool_names, convert_tools, create_placeholder_tool};

use super::types::MessagesRequest;

/// 将 Anthropic 请求转换为 Kiro 请求
pub fn convert_request(
    req: &MessagesRequest,
    compression_config: &CompressionConfig,
    forced_conversation_id: Option<&str>,
) -> Result<ConversionResult, ConversionError> {
    // 1. 映射模型（已知模型走映射表，未知模型直接透传给 Kiro 后端）
    let model_id = match map_model(&req.model) {
        Some(id) => id,
        None => {
            tracing::info!(model = %req.model, "未知模型，直接透传给上游");
            req.model.clone()
        }
    };

    // 2. 检查消息列表
    if req.messages.is_empty() {
        return Err(ConversionError::EmptyMessages);
    }

    // 2.5. 预处理 prefill：如果末尾是 assistant，静默丢弃并截断到最后一条 user
    // 原因：Claude 4.x 已弃用 assistant prefill，Kiro API 也不支持
    let messages: &[_] = if req
        .messages
        .last()
        .map(|m| m.role != "user")
        .unwrap_or(false)
    {
        tracing::info!("检测到末尾 assistant 消息（prefill），静默丢弃，回退到最后一条 user 消息");
        let last_user_idx = req
            .messages
            .iter()
            .rposition(|m| m.role == "user")
            .ok_or(ConversionError::EmptyMessages)?;
        &req.messages[..=last_user_idx]
    } else {
        &req.messages
    };

    // 2.6. 验证最后一条消息内容不为空
    // 检查最后一条消息（经过 prefill 处理后）是否有有效内容
    let last_message = messages.last().unwrap();
    let has_valid_content = match &last_message.content {
        serde_json::Value::String(s) => !s.trim().is_empty(),
        serde_json::Value::Array(arr) => arr.iter().any(|item| {
            if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                match block.block_type.as_str() {
                    "text" => block.text.as_ref().is_some_and(|t| !t.trim().is_empty()),
                    "image" | "tool_use" | "tool_result" | "document" => true,
                    _ => false,
                }
            } else {
                false
            }
        }),
        _ => false,
    };
    if !has_valid_content {
        tracing::warn!("最后一条消息内容为空（仅包含空白文本或无内容）");
        return Err(ConversionError::EmptyMessageContent);
    }

    // 3. 生成会话 ID 和代理 ID
    //
    // 优先级:
    //   (a) metadata.user_id 含 session UUID → 直接取
    //   (b) 否则用 history 前缀（除最后一条 user）SHA-256 指纹做 fallback。
    //       proxy 是无状态的，但同一会话的"已经发生过的对话"对每次请求都稳定；
    //       同一上下文 → 同一 conversation_id → 同一 v5 派生的 agentContinuationId。
    //   (c) 单轮且无 metadata 时（history 为空），退化为 Uuid::v4。
    //
    // 重要：旧实现在 (a) 失败时直接 Uuid::v4，导致 Anthropic SDK 默认配置
    // （不传 metadata.user_id）下每请求一个新 conversation_id，
    // patch #6 (agentContinuationId v5 派生) 完全空操作 → 多轮 prefix cache 失效。
    let conversation_id = if let Some(forced) = forced_conversation_id {
        tracing::debug!(
            forced_conversation_id = forced,
            "使用 CrossRequestCache 注入的 conversation_id"
        );
        forced.to_string()
    } else {
        req.metadata
            .as_ref()
            .and_then(|m| m.user_id.as_ref())
            .and_then(|user_id| extract_session_id(user_id))
            .or_else(|| messages_history_fingerprint(messages))
            .unwrap_or_else(|| Uuid::new_v4().to_string())
    };
    // kiro-cli 2.3.0 multi-turn capture (gar-body-1.json & gar-body-3.json)
    // shows the same `agentContinuationId` across every turn of one session —
    // it's a session-stable ID, not per-request. We derive it deterministically
    // from `conversation_id` so repeat requests under the same session reuse it.
    let agent_continuation_id =
        Uuid::new_v5(&Uuid::NAMESPACE_DNS, conversation_id.as_bytes()).to_string();

    // 4. 确定触发类型
    let chat_trigger_type = determine_chat_trigger_type(req);

    // 4.5. 统计图片总数（用于决定压缩策略，基于截断后的 messages）
    let total_image_count: usize = messages
        .iter()
        .map(|msg| count_images_in_content(&msg.content))
        .sum();

    // 4.6. 初始化图片配额（所有消息合计不超过 MAX_TOTAL_IMAGES）
    let mut remaining_image_budget = MAX_TOTAL_IMAGES;

    // 5. 处理最后一条消息作为 current_message（经过 prefill 预处理，末尾必为 user）
    // 先处理 currentMessage 以优先保留当前用户输入的图片
    let last_message = messages.last().unwrap();
    let (text_content, images, tool_results) = process_message_content(
        &last_message.content,
        compression_config,
        total_image_count,
        &mut remaining_image_budget,
    )?;

    // 6. 转换工具定义（超长名称自动缩短并记录映射）
    let mut tool_name_map = HashMap::new();
    let mut tools = convert_tools(
        &req.tools,
        compression_config.tool_description_max_chars,
        &mut tool_name_map,
    );

    // 7. 构建历史消息（需要先构建，以便收集历史中使用的工具）
    // history 使用 currentMessage 消耗后的剩余图片配额
    let mut history = build_history(
        req,
        messages,
        BuildHistoryContext {
            model_id: &model_id,
            compression_config,
            total_image_count,
            is_agentic: is_agentic_model(&req.model),
            remaining_image_budget: &mut remaining_image_budget,
            tool_name_map: &mut tool_name_map,
        },
    )?;

    // 8. 验证并过滤 tool_use/tool_result 配对
    // 移除孤立的 tool_result（没有对应的 tool_use）
    // 同时返回孤立的 tool_use_id 集合，用于后续清理
    let (validated_tool_results, orphaned_tool_use_ids) =
        validate_tool_pairing(&history, &tool_results);

    // 9. 从历史中移除孤立的 tool_use（Kiro API 要求 tool_use 必须有对应的 tool_result）
    remove_orphaned_tool_uses(&mut history, &orphaned_tool_use_ids);

    // 10. 收集历史中使用的工具名称，为缺失的工具生成占位符定义
    // Kiro API 要求：历史消息中引用的工具必须在 tools 列表中有定义
    // 注意：Kiro 匹配工具名称时忽略大小写，所以这里也需要忽略大小写比较
    let history_tool_names = collect_history_tool_names(&history);
    let mut existing_tool_names: std::collections::HashSet<_> = tools
        .iter()
        .map(|t| t.tool_specification.name.to_lowercase())
        .collect();

    for tool_name in history_tool_names {
        let lower = tool_name.to_lowercase();
        if !existing_tool_names.contains(&lower) {
            tools.push(create_placeholder_tool(&tool_name));
            existing_tool_names.insert(lower);
        }
    }

    // 10.5. 工具压缩：在所有工具（含 placeholder）就绪后执行
    tools = super::tool_compression::compress_tools_if_needed(&tools);

    // 10.6. 工具统计诊断日志
    {
        let original_tool_count = req.tools.as_ref().map(|t| t.len()).unwrap_or(0);
        let placeholder_count = tools.len().saturating_sub(original_tool_count);

        // 大小写不敏感的重复检测
        let mut name_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for t in &tools {
            *name_counts
                .entry(t.tool_specification.name.to_lowercase())
                .or_insert(0) += 1;
        }
        let duplicates: Vec<_> = name_counts
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(name, count)| format!("{}(x{})", name, count))
            .collect();

        if !duplicates.is_empty() {
            tracing::warn!(
                tool_count = tools.len(),
                duplicates = ?duplicates,
                "检测到重复工具名称（大小写不敏感）"
            );
        }
        tracing::info!(
            tool_count = tools.len(),
            placeholder_count = placeholder_count,
            "工具定义统计"
        );
    }

    // 11. 构建 UserInputMessageContext
    let mut context = UserInputMessageContext::new();
    if !tools.is_empty() {
        context = context.with_tools(tools);
    }
    let has_tool_results = !validated_tool_results.is_empty();
    if has_tool_results {
        context = context.with_tool_results(validated_tool_results);
    }

    // 12. 构建当前消息
    // 保留文本内容，即使有工具结果也不丢弃用户文本
    let content = non_empty_content_or_space(text_content, !images.is_empty() || has_tool_results);
    // current_message 是请求主体，必须保留；若文本为空且无非文本载荷，最终兜底
    let content = if content.trim().is_empty() && images.is_empty() && !has_tool_results {
        tracing::warn!("currentMessage content 为空，已使用占位符修复");
        ".".to_string()
    } else {
        content
    };

    let mut user_input = UserInputMessage::new(content, &model_id)
        .with_context(context)
        .with_origin("AI_EDITOR");

    if !images.is_empty() {
        user_input = user_input.with_images(images);
    }

    let current_message = CurrentMessage::new(user_input);

    // 12.5. 图片统计日志
    {
        let actual_image_count = MAX_TOTAL_IMAGES - remaining_image_budget;
        if actual_image_count > 0 || total_image_count > 0 {
            tracing::info!(
                source_image_count = total_image_count,
                actual_image_count = actual_image_count,
                images_dropped = total_image_count.saturating_sub(actual_image_count),
                budget_remaining = remaining_image_budget,
                "图片统计"
            );
        }
    }

    // 13. 构建 ConversationState
    let mut conversation_state = ConversationState::new(conversation_id)
        .with_agent_continuation_id(agent_continuation_id)
        .with_agent_task_type("vibe")
        .with_chat_trigger_type(chat_trigger_type)
        .with_current_message(current_message)
        .with_history(history);

    // 14. 执行输入压缩
    let compression_stats = if compression_config.enabled {
        let stats = super::compressor::compress(&mut conversation_state, compression_config);
        if stats.total_saved() > 0 || stats.history_turns_removed > 0 {
            Some(stats)
        } else {
            None
        }
    } else {
        None
    };

    if !tool_name_map.is_empty() {
        tracing::info!("工具名称映射: {} 个超长名称已缩短", tool_name_map.len());
    }

    Ok(ConversionResult {
        conversation_state,
        compression_stats,
        tool_name_map,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kiro::model::requests::conversation::{
        AssistantMessage, HistoryAssistantMessage, HistoryUserMessage, Message, UserMessage,
    };
    use crate::kiro::model::requests::tool::ToolResult;
    use crate::model::config::CompressionConfig;
    use history::convert_assistant_message;
    use model::*;
    use schema::normalize_json_schema;
    use system::{
        ThinkingSchemaPath, build_additional_model_request_fields,
        extract_thinking_config_from_schema, generate_thinking_prefix,
        output_config_thinking_schema, wrap_system_for_history,
    };
    use tools::{collect_history_tool_names, create_placeholder_tool};

    #[test]
    fn test_map_model_sonnet() {
        assert_eq!(
            map_model("claude-sonnet-4-20250514").unwrap(),
            KIRO_MODEL_SONNET_4_5
        );
        assert_eq!(
            map_model("claude-3-5-sonnet-20241022").unwrap(),
            KIRO_MODEL_SONNET_4_5
        );
        assert_eq!(
            map_model("claude-sonnet-4-6").unwrap(),
            KIRO_MODEL_SONNET_4_6
        );
        assert_eq!(
            map_model("claude-sonnet-4.6").unwrap(),
            KIRO_MODEL_SONNET_4_6
        );
    }

    #[test]
    fn test_map_model_opus() {
        assert_eq!(
            map_model("claude-opus-4-20250514").unwrap(),
            KIRO_MODEL_OPUS_4_6
        );
        assert_eq!(
            map_model("claude-opus-4-20260206").unwrap(),
            KIRO_MODEL_OPUS_4_6
        );
        assert_eq!(
            map_model("claude-opus-4-5-20250514").unwrap(),
            KIRO_MODEL_OPUS_4_5
        );
        assert_eq!(map_model("claude-opus-4.5").unwrap(), KIRO_MODEL_OPUS_4_5);
        assert_eq!(map_model("claude-opus-4-6").unwrap(), KIRO_MODEL_OPUS_4_6);
        assert_eq!(map_model("claude-opus-4-7").unwrap(), KIRO_MODEL_OPUS_4_7);
        assert_eq!(map_model("claude-opus-4.7").unwrap(), KIRO_MODEL_OPUS_4_7);
        assert_eq!(map_model("claude-opus-4-8").unwrap(), KIRO_MODEL_OPUS_4_8);
        assert_eq!(map_model("claude-opus-4.8").unwrap(), KIRO_MODEL_OPUS_4_8);
    }

    #[test]
    fn test_map_model_haiku() {
        assert_eq!(
            map_model("claude-haiku-4-20250514").unwrap(),
            KIRO_MODEL_HAIKU_4_5
        );
        assert_eq!(
            map_model("claude-haiku-4-5-20251001").unwrap(),
            KIRO_MODEL_HAIKU_4_5
        );
    }

    #[test]
    fn test_map_model_gpt4_maps_to_sonnet() {
        assert_eq!(map_model("gpt-4").unwrap(), KIRO_MODEL_SONNET_4_5);
    }

    #[test]
    fn test_map_model_unknown_returns_none() {
        assert!(map_model("deepseek-v3.2").is_none());
        assert!(map_model("qwen3-coder-next").is_none());
    }

    #[test]
    fn test_map_model_thinking_suffixes() {
        assert_eq!(
            map_model("claude-sonnet-4-5-20250929-thinking"),
            Some(KIRO_MODEL_SONNET_4_5.to_string())
        );
        assert_eq!(
            map_model("claude-sonnet-4-6-thinking"),
            Some(KIRO_MODEL_SONNET_4_6.to_string())
        );
        assert_eq!(
            map_model("claude-opus-4-5-20251101-thinking"),
            Some(KIRO_MODEL_OPUS_4_5.to_string())
        );
        assert_eq!(
            map_model("claude-opus-4-6-thinking"),
            Some(KIRO_MODEL_OPUS_4_6.to_string())
        );
        assert_eq!(
            map_model("claude-opus-4-7-thinking"),
            Some(KIRO_MODEL_OPUS_4_7.to_string())
        );
        assert_eq!(
            map_model("claude-haiku-4-5-20251001-thinking"),
            Some(KIRO_MODEL_HAIKU_4_5.to_string())
        );
    }

    #[test]
    fn test_map_model_agentic_suffixes() {
        assert_eq!(
            map_model("claude-sonnet-4-6-agentic"),
            Some(KIRO_MODEL_SONNET_4_6.to_string())
        );
        assert_eq!(
            map_model("claude-sonnet-4-5-20250929-agentic"),
            Some(KIRO_MODEL_SONNET_4_5.to_string())
        );
        assert_eq!(
            map_model("claude-opus-4-6-agentic"),
            Some(KIRO_MODEL_OPUS_4_6.to_string())
        );
        assert_eq!(
            map_model("claude-opus-4-7-agentic"),
            Some(KIRO_MODEL_OPUS_4_7.to_string())
        );
        assert_eq!(
            map_model("claude-opus-4-5-20251101-agentic"),
            Some(KIRO_MODEL_OPUS_4_5.to_string())
        );
        assert_eq!(
            map_model("claude-haiku-4-5-20251001-agentic"),
            Some(KIRO_MODEL_HAIKU_4_5.to_string())
        );
    }

    #[test]
    fn test_map_model_versioned_entries_from_models_endpoint() {
        let supported_models = [
            ("claude-sonnet-4-6", KIRO_MODEL_SONNET_4_6),
            ("claude-sonnet-4-6-thinking", KIRO_MODEL_SONNET_4_6),
            ("claude-sonnet-4-6-agentic", KIRO_MODEL_SONNET_4_6),
            ("claude-sonnet-4-5-20250929", KIRO_MODEL_SONNET_4_5),
            ("claude-sonnet-4-5-20250929-thinking", KIRO_MODEL_SONNET_4_5),
            ("claude-sonnet-4-5-20250929-agentic", KIRO_MODEL_SONNET_4_5),
            ("claude-opus-4-5-20251101", KIRO_MODEL_OPUS_4_5),
            ("claude-opus-4-5-20251101-thinking", KIRO_MODEL_OPUS_4_5),
            ("claude-opus-4-5-20251101-agentic", KIRO_MODEL_OPUS_4_5),
            ("claude-opus-4-6", KIRO_MODEL_OPUS_4_6),
            ("claude-opus-4-6-thinking", KIRO_MODEL_OPUS_4_6),
            ("claude-opus-4-6-agentic", KIRO_MODEL_OPUS_4_6),
            ("claude-opus-4-7", KIRO_MODEL_OPUS_4_7),
            ("claude-opus-4-7-thinking", KIRO_MODEL_OPUS_4_7),
            ("claude-opus-4-7-agentic", KIRO_MODEL_OPUS_4_7),
            ("claude-opus-4-8", KIRO_MODEL_OPUS_4_8),
            ("claude-opus-4-8-thinking", KIRO_MODEL_OPUS_4_8),
            ("claude-opus-4-8-agentic", KIRO_MODEL_OPUS_4_8),
            ("claude-haiku-4-5-20251001", KIRO_MODEL_HAIKU_4_5),
            ("claude-haiku-4-5-20251001-thinking", KIRO_MODEL_HAIKU_4_5),
            ("claude-haiku-4-5-20251001-agentic", KIRO_MODEL_HAIKU_4_5),
        ];

        for (input, expected) in supported_models {
            assert_eq!(map_model(input), Some(expected.to_string()), "{input}");
        }
    }

    #[test]
    fn test_determine_chat_trigger_type() {
        // 无工具时返回 MANUAL
        let req = MessagesRequest {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 1024,
            messages: vec![],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };
        assert_eq!(system::determine_chat_trigger_type(&req), "MANUAL");
    }

    #[test]
    fn test_collect_history_tool_names() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 创建包含工具使用的历史消息
        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
            ToolUseEntry::new("tool-2", "write")
                .with_input(serde_json::json!({"path": "/out.txt"})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        let tool_names = collect_history_tool_names(&history);
        assert_eq!(tool_names.len(), 2);
        assert!(tool_names.contains(&"read".to_string()));
        assert!(tool_names.contains(&"write".to_string()));
    }

    #[test]
    fn test_create_placeholder_tool() {
        let tool = create_placeholder_tool("my_custom_tool");

        assert_eq!(tool.tool_specification.name, "my_custom_tool");
        assert!(!tool.tool_specification.description.is_empty());

        // 验证 JSON 序列化正确
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"name\":\"my_custom_tool\""));
    }

    #[test]
    fn test_history_tools_added_to_tools_list() {
        use crate::anthropic::types::Message as AnthropicMessage;

        // 创建一个请求，历史中有工具使用，但 tools 列表为空
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("Read the file"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "text", "text": "I'll read the file."},
                        {"type": "tool_use", "id": "tool-1", "name": "read", "input": {"path": "/test.txt"}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "tool-1", "content": "file content"}
                    ]),
                },
            ],
            stream: false,
            system: None,
            tools: None, // 没有提供工具定义
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();

        // 验证 tools 列表中包含了历史中使用的工具的占位符定义
        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;

        assert!(!tools.is_empty(), "tools 列表不应为空");
        assert!(
            tools.iter().any(|t| t.tool_specification.name == "read"),
            "tools 列表应包含 'read' 工具的占位符定义"
        );
    }

    /// Round 4 regression: agentContinuationId must be deterministic from
    /// conversationId — multi-turn captures of kiro-cli 2.3.0 show the same
    /// continuation UUID across every turn of one session. The proxy is
    /// stateless so we derive it via UUID v5 on conversation_id.
    #[test]
    fn test_wrap_system_for_history_matches_kiro_cli_format() {
        // Verify the wire-aligned wrapper format.
        let wrapped = wrap_system_for_history("You are terse.");
        assert_eq!(
            wrapped,
            "--- CONTEXT ENTRY BEGIN ---\n--- CONTEXT ENTRY END ---\n\nFollow this instruction: You are terse."
        );
    }

    /// Round 5 regression: wrap_system_for_history must be idempotent when the
    /// input already starts with the wrapper or `Follow this instruction:`.
    /// Pre-fix double-wrapping broke the prefix-cache key.
    #[test]
    fn test_wrap_system_for_history_idempotent() {
        let already_wrapped =
            "--- CONTEXT ENTRY BEGIN ---\n--- CONTEXT ENTRY END ---\n\nFollow this instruction: hi";
        assert_eq!(wrap_system_for_history(already_wrapped), already_wrapped);
        let with_follow_prefix = "Follow this instruction: hi";
        assert_eq!(
            wrap_system_for_history(with_follow_prefix),
            with_follow_prefix
        );
        // Whitespace at the head must not break detection.
        let leading_ws = "  Follow this instruction: hi";
        assert_eq!(wrap_system_for_history(leading_ws), leading_ws);
    }

    /// Round 7 regression: messages_history_fingerprint is keyed on the
    /// **first message only** (post-Round 7 redesign). Same first message →
    /// same fingerprint across ALL turns of the same session, even as
    /// history grows. Different first message → different fingerprint.
    #[test]
    fn test_messages_history_fingerprint_stability() {
        use crate::anthropic::types::Message;
        let mk = |role: &str, content: &str| Message {
            role: role.to_string(),
            content: serde_json::Value::String(content.to_string()),
        };
        // Empty messages → None (let caller fall back to v4).
        let empty: Vec<Message> = vec![];
        assert!(messages_history_fingerprint(&empty).is_none());

        // Single message → returns Some (Round 7: first-message keying).
        let single = vec![mk("user", "hi")];
        let fp_single = messages_history_fingerprint(&single).unwrap();

        // Turn 2 of same session (first message unchanged, new turns appended)
        // → same fingerprint as turn 1.
        let turn2 = vec![mk("user", "hi"), mk("assistant", "yo"), mk("user", "again")];
        let fp_turn2 = messages_history_fingerprint(&turn2).unwrap();
        assert_eq!(
            fp_single, fp_turn2,
            "first-message anchor must persist across turns"
        );

        // Turn 5 of same session — history grew further, fingerprint unchanged.
        let turn5 = vec![
            mk("user", "hi"),
            mk("assistant", "yo"),
            mk("user", "again"),
            mk("assistant", "ok"),
            mk("user", "more"),
            mk("assistant", "done"),
            mk("user", "current"),
        ];
        let fp_turn5 = messages_history_fingerprint(&turn5).unwrap();
        assert_eq!(
            fp_single, fp_turn5,
            "same first-message → same fingerprint regardless of history length"
        );

        // Different session (different first message) → different fingerprint.
        let other = vec![mk("user", "different first message")];
        let fp_other = messages_history_fingerprint(&other).unwrap();
        assert_ne!(fp_single, fp_other);
    }

    #[test]
    fn test_agent_continuation_id_deterministic_from_conversation_id() {
        let conv = "11111111-2222-3333-4444-555555555555";
        let id1 = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, conv.as_bytes()).to_string();
        let id2 = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, conv.as_bytes()).to_string();
        assert_eq!(id1, id2, "same conversation_id → same agentContinuationId");
        let id_other =
            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"different-conv").to_string();
        assert_ne!(id1, id_other, "different conversation_id → different id");
    }

    #[test]
    fn test_extract_session_id_valid() {
        // 测试有效的 user_id 格式
        let user_id = "user_0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd_account__session_8bb5523b-ec7c-4540-a9ca-beb6d79f1552";
        let session_id = extract_session_id(user_id);
        assert_eq!(
            session_id,
            Some("8bb5523b-ec7c-4540-a9ca-beb6d79f1552".to_string())
        );
    }

    #[test]
    fn test_extract_session_id_no_session() {
        // 测试没有 session 的 user_id
        let user_id = "user_0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd";
        let session_id = extract_session_id(user_id);
        assert_eq!(session_id, None);
    }

    #[test]
    fn test_extract_session_id_invalid_uuid() {
        // 测试无效的 UUID 格式
        let user_id = "user_xxx_session_invalid-uuid";
        let session_id = extract_session_id(user_id);
        assert_eq!(session_id, None);
    }

    #[test]
    fn test_extract_session_id_json_format() {
        // 测试 JSON 格式的 user_id
        let user_id = r#"{"device_id":"0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd","account_uuid":"","session_id":"8bb5523b-ec7c-4540-a9ca-beb6d79f1552"}"#;
        let session_id = extract_session_id(user_id);
        assert_eq!(
            session_id,
            Some("8bb5523b-ec7c-4540-a9ca-beb6d79f1552".to_string())
        );
    }

    #[test]
    fn test_extract_session_id_json_invalid_session() {
        // 测试 JSON 格式但 session_id 不是有效 UUID
        let user_id = r#"{"device_id":"abc","session_id":"not-a-uuid"}"#;
        let session_id = extract_session_id(user_id);
        assert_eq!(session_id, None);
    }

    #[test]
    fn test_convert_request_with_session_metadata() {
        use crate::anthropic::types::{Message as AnthropicMessage, Metadata};

        // 测试带有 metadata 的请求，应该使用 session UUID 作为 conversationId
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: Some(Metadata {
                user_id: Some(
                    "user_0dede55c6dcc4a11a30bbb5e7f22e6fdf86cdeba3820019cc27612af4e1243cd_account__session_a0662283-7fd3-4399-a7eb-52b9a717ae88".to_string(),
                ),
            }),
        };

        let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();
        assert_eq!(
            result.conversation_state.conversation_id,
            "a0662283-7fd3-4399-a7eb-52b9a717ae88"
        );
    }

    #[test]
    fn test_convert_request_without_metadata() {
        use crate::anthropic::types::Message as AnthropicMessage;

        // 测试没有 metadata 的请求，应该生成新的 UUID
        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("Hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();
        // 验证生成的是有效的 UUID 格式
        assert_eq!(result.conversation_state.conversation_id.len(), 36);
        assert_eq!(
            result
                .conversation_state
                .conversation_id
                .chars()
                .filter(|c| *c == '-')
                .count(),
            4
        );
    }

    #[test]
    fn test_validate_tool_pairing_orphaned_result() {
        // 测试孤立的 tool_result 被过滤
        // 历史中没有 tool_use，但 tool_results 中有 tool_result
        let history = vec![
            Message::User(HistoryUserMessage::new("Hello", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage::new("Hi there!")),
        ];

        let tool_results = vec![ToolResult::success("orphan-123", "some result")];

        let (filtered, _) = validate_tool_pairing(&history, &tool_results);

        // 孤立的 tool_result 应该被过滤掉
        assert!(filtered.is_empty(), "孤立的 tool_result 应该被过滤");
    }

    #[test]
    fn test_validate_tool_pairing_orphaned_use() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试孤立的 tool_use（有 tool_use 但没有对应的 tool_result）
        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-orphan", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        // 没有 tool_result
        let tool_results: Vec<ToolResult> = vec![];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        // 结果应该为空（因为没有 tool_result）
        // 同时应该返回孤立的 tool_use_id
        assert!(filtered.is_empty());
        assert!(orphaned.contains("tool-orphan"));
    }

    #[test]
    fn test_validate_tool_pairing_valid() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试正常配对的情况
        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        let tool_results = vec![ToolResult::success("tool-1", "file content")];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        // 配对成功，应该保留，无孤立
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].tool_use_id, "tool-1");
        assert!(orphaned.is_empty());
    }

    #[test]
    fn test_validate_tool_pairing_mixed() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        // 测试混合情况：部分配对成功，部分孤立
        let mut assistant_msg = AssistantMessage::new("I'll use two tools.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read").with_input(serde_json::json!({})),
            ToolUseEntry::new("tool-2", "write").with_input(serde_json::json!({})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new("Do something", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        // tool_results: tool-1 配对，tool-3 孤立
        let tool_results = vec![
            ToolResult::success("tool-1", "result 1"),
            ToolResult::success("tool-3", "orphan result"), // 孤立
        ];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        // 只有 tool-1 应该保留
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].tool_use_id, "tool-1");
        // tool-2 是孤立的 tool_use（无 result），tool-3 是孤立的 tool_result
        assert!(orphaned.contains("tool-2"));
    }

    #[test]
    fn test_validate_tool_pairing_history_already_paired() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        let mut assistant_msg1 = AssistantMessage::new("I'll read the file.");
        assistant_msg1 = assistant_msg1.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        let mut user_msg_with_result = UserMessage::new("", "claude-sonnet-4.5");
        let mut ctx = UserInputMessageContext::new();
        ctx = ctx.with_tool_results(vec![ToolResult::success("tool-1", "file content")]);
        user_msg_with_result = user_msg_with_result.with_context(ctx);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg1,
            }),
            Message::User(HistoryUserMessage {
                user_input_message: user_msg_with_result,
            }),
            Message::Assistant(HistoryAssistantMessage::new("The file contains...")),
        ];

        let tool_results: Vec<ToolResult> = vec![];

        let (filtered, orphaned) = validate_tool_pairing(&history, &tool_results);

        assert!(filtered.is_empty());
        assert!(orphaned.is_empty());
    }

    #[test]
    fn test_validate_tool_pairing_duplicate_result() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        let mut assistant_msg = AssistantMessage::new("I'll read the file.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read")
                .with_input(serde_json::json!({"path": "/test.txt"})),
        ]);

        let mut user_msg_with_result = UserMessage::new("", "claude-sonnet-4.5");
        let mut ctx = UserInputMessageContext::new();
        ctx = ctx.with_tool_results(vec![ToolResult::success("tool-1", "file content")]);
        user_msg_with_result = user_msg_with_result.with_context(ctx);

        let history = vec![
            Message::User(HistoryUserMessage::new(
                "Read the file",
                "claude-sonnet-4.5",
            )),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
            Message::User(HistoryUserMessage {
                user_input_message: user_msg_with_result,
            }),
            Message::Assistant(HistoryAssistantMessage::new("Done")),
        ];

        let tool_results = vec![ToolResult::success("tool-1", "file content again")];

        let (filtered, _) = validate_tool_pairing(&history, &tool_results);

        assert!(filtered.is_empty(), "重复的 tool_result 应该被过滤");
    }

    #[test]
    fn test_convert_assistant_message_tool_use_only() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let msg = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "tool_use", "id": "toolu_01ABC", "name": "read_file", "input": {"path": "/test.txt"}}
            ]),
        };

        let result = convert_assistant_message(&msg, &mut HashMap::new()).expect("应该成功转换");

        assert!(
            result.assistant_response_message.content.is_empty(),
            "仅 tool_use 时转换阶段不应主动补 '.'"
        );

        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("应该有 tool_uses");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_01ABC");
        assert_eq!(tool_uses[0].name, "read_file");
    }

    #[test]
    fn test_convert_assistant_message_with_text_and_tool_use() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let msg = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": "Let me read that file for you."},
                {"type": "tool_use", "id": "toolu_02XYZ", "name": "read_file", "input": {"path": "/data.json"}}
            ]),
        };

        let result = convert_assistant_message(&msg, &mut HashMap::new()).expect("应该成功转换");

        assert_eq!(
            result.assistant_response_message.content,
            "Let me read that file for you."
        );

        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("应该有 tool_uses");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_02XYZ");
    }

    #[test]
    fn test_convert_assistant_message_web_search_tool_result() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let msg = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "server_tool_use", "id": "srvtoolu_01ABC", "name": "web_search", "input": {"query": "rust async"}},
                {
                    "type": "web_search_tool_result",
                    "content": [
                        {
                            "type": "web_search_result",
                            "title": "Async in Rust",
                            "url": "https://rust-lang.org/async",
                            "encrypted_content": "Rust async/await guide.",
                            "page_age": "January 1, 2025"
                        },
                        {"type": "web_search_result", "title": "", "url": "https://example.com/no-title",
                         "encrypted_content": "", "page_age": null}
                    ]
                }
            ]),
        };

        let result = convert_assistant_message(&msg, &mut HashMap::new()).expect("应该成功转换");
        let content = &result.assistant_response_message.content;

        assert!(
            content.contains("Async in Rust: https://rust-lang.org/async"),
            "有 title 时应输出 'title: url'"
        );
        assert!(content.contains("Date: January 1, 2025"), "page_age 应保留");
        assert!(
            content.contains("Rust async/await guide."),
            "snippet 应保留"
        );
        assert!(
            content.contains("https://example.com/no-title"),
            "title 为空时应输出纯 URL"
        );
        assert!(
            !content.contains("srvtoolu_01ABC"),
            "server_tool_use 应被忽略"
        );
    }

    #[test]
    fn test_convert_assistant_message_web_search_result_control_chars() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let msg = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {
                    "type": "web_search_tool_result",
                    "content": [
                        {"type": "web_search_result", "title": "Title\nWith\tControl", "url": "https://example.com"}
                    ]
                }
            ]),
        };

        let result = convert_assistant_message(&msg, &mut HashMap::new()).expect("应该成功转换");
        let content = &result.assistant_response_message.content;

        assert!(!content.contains('\t'), "tab 字符应被过滤");
        assert!(content.contains("https://example.com"), "URL 应保留");
    }

    #[test]
    fn test_convert_tools_filters_web_search() {
        use crate::anthropic::types::Tool as AnthropicTool;
        use std::collections::HashMap;

        let tools = vec![
            AnthropicTool {
                tool_type: Some("web_search_20250305".to_string()),
                name: "web_search".to_string(),
                description: String::new(),
                input_schema: HashMap::new(),
                max_uses: Some(8),
                cache_control: None,
            },
            AnthropicTool {
                tool_type: None,
                name: "read_file".to_string(),
                description: "Read a file from disk".to_string(),
                input_schema: {
                    let mut schema = HashMap::new();
                    schema.insert("type".to_string(), serde_json::json!("object"));
                    schema
                },
                max_uses: None,
                cache_control: None,
            },
        ];

        let converted = tools::convert_tools(&Some(tools), 4000, &mut HashMap::new());

        assert_eq!(converted.len(), 1, "web_search 应该被过滤");
        assert_eq!(
            converted[0].tool_specification.name, "read_file",
            "只应保留 read_file 工具"
        );
    }

    #[test]
    fn test_convert_tools_filters_all_web_search_variants() {
        use crate::anthropic::types::Tool as AnthropicTool;
        use std::collections::HashMap;

        let tools = vec![
            AnthropicTool {
                tool_type: Some("web_search_20250305".to_string()),
                name: "web_search".to_string(),
                description: String::new(),
                input_schema: HashMap::new(),
                max_uses: Some(8),
                cache_control: None,
            },
            AnthropicTool {
                tool_type: Some("web_search_20260101".to_string()),
                name: "web_search".to_string(),
                description: String::new(),
                input_schema: HashMap::new(),
                max_uses: Some(10),
                cache_control: None,
            },
        ];

        let converted = tools::convert_tools(&Some(tools), 4000, &mut HashMap::new());

        assert!(converted.is_empty(), "所有 web_search 变体都应被过滤");
    }

    #[test]
    fn test_convert_tools_fills_empty_description_and_normalizes_schema() {
        use crate::anthropic::types::{Message as AnthropicMessage, Tool as AnthropicTool};
        use std::collections::HashMap;

        let mut input_schema = HashMap::new();
        input_schema.insert("type".to_string(), serde_json::json!("object"));

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 128,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hi"),
            }],
            stream: false,
            system: None,
            tools: Some(vec![AnthropicTool {
                tool_type: None,
                name: "mcp__ida-pro-mcp__patch_address_assembles".to_string(),
                description: "".to_string(),
                input_schema,
                max_uses: None,
                cache_control: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();
        let tools = &result
            .conversation_state
            .current_message
            .user_input_message
            .user_input_message_context
            .tools;

        let tool = tools
            .iter()
            .find(|t| t.tool_specification.name == "mcp__ida-pro-mcp__patch_address_assembles")
            .expect("转换后应包含该工具");

        assert!(
            !tool.tool_specification.description.trim().is_empty(),
            "转换后的工具描述不应为空"
        );
        assert!(
            tool.tool_specification.input_schema.json["$schema"].is_null(),
            "$schema 缺失时不应自动注入"
        );
        assert_eq!(tool.tool_specification.input_schema.json["type"], "object");
    }

    #[test]
    fn test_current_message_content_is_non_empty_when_only_tool_result() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 128,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("do it"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_use", "id": "tool-1", "name": "read", "input": {"path": "/tmp/a"}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "tool-1", "content": "ok"}
                    ]),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();
        let content = &result
            .conversation_state
            .current_message
            .user_input_message
            .content;

        assert!(
            content.is_empty(),
            "仅有效 tool_result 的 current user 消息不应在早期转换阶段补 '.'"
        );
    }

    #[test]
    fn test_history_user_message_content_is_non_empty_when_only_tool_result() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 128,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("do it"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_use", "id": "tool-1", "name": "read", "input": {"path": "/tmp/a"}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "tool-1", "content": "ok"}
                    ]),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!("done"),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("next"),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();

        let mut found = false;
        for msg in &result.conversation_state.history {
            let Message::User(user_msg) = msg else {
                continue;
            };
            let ctx = &user_msg.user_input_message.user_input_message_context;
            if ctx.tool_results.is_empty() {
                continue;
            }
            found = true;
            assert!(
                user_msg.user_input_message.content.is_empty(),
                "history 中仅含有效 tool_result 的 user 消息不应在早期转换阶段补 '.'"
            );
        }
        assert!(found, "测试数据应在 history 中包含 tool_results");
    }

    #[test]
    fn test_current_message_content_is_non_empty_when_tool_result_filtered_as_orphan() {
        use crate::anthropic::types::{Message as AnthropicMessage, Tool as AnthropicTool};
        use std::collections::HashMap;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 128,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("请读取配置"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_use", "id": "tooluse_valid_1", "name": "read_file", "input": {"path": "/tmp/a"}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "toolu_orphan_1", "content": "ok"}
                    ]),
                },
            ],
            stream: false,
            system: None,
            tools: Some(vec![AnthropicTool {
                tool_type: None,
                name: "read_file".to_string(),
                description: "read".to_string(),
                input_schema: HashMap::new(),
                max_uses: None,
                cache_control: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();

        assert!(
            result
                .conversation_state
                .current_message
                .user_input_message
                .user_input_message_context
                .tool_results
                .is_empty(),
            "孤立 tool_result 应被过滤"
        );

        assert_eq!(
            result
                .conversation_state
                .current_message
                .user_input_message
                .content,
            "."
        );
    }

    #[test]
    fn test_remove_orphaned_tool_uses() {
        use crate::kiro::model::requests::tool::ToolUseEntry;
        use history::remove_orphaned_tool_uses;

        let mut assistant_msg = AssistantMessage::new("I'll use multiple tools.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read").with_input(serde_json::json!({})),
            ToolUseEntry::new("tool-2", "write").with_input(serde_json::json!({})),
            ToolUseEntry::new("tool-3", "delete").with_input(serde_json::json!({})),
        ]);

        let mut history = vec![
            Message::User(HistoryUserMessage::new("Do something", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        let mut orphaned = std::collections::HashSet::new();
        orphaned.insert("tool-1".to_string());
        orphaned.insert("tool-3".to_string());

        remove_orphaned_tool_uses(&mut history, &orphaned);

        if let Message::Assistant(ref assistant_msg) = history[1] {
            let tool_uses = assistant_msg
                .assistant_response_message
                .tool_uses
                .as_ref()
                .expect("应该还有 tool_uses");
            assert_eq!(tool_uses.len(), 1);
            assert_eq!(tool_uses[0].tool_use_id, "tool-2");
        } else {
            panic!("应该是 Assistant 消息");
        }
    }

    #[test]
    fn test_remove_orphaned_tool_uses_all_removed() {
        use crate::kiro::model::requests::tool::ToolUseEntry;
        use history::remove_orphaned_tool_uses;

        let mut assistant_msg = AssistantMessage::new("I'll use a tool.");
        assistant_msg = assistant_msg.with_tool_uses(vec![
            ToolUseEntry::new("tool-1", "read").with_input(serde_json::json!({})),
        ]);

        let mut history = vec![
            Message::User(HistoryUserMessage::new("Do something", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: assistant_msg,
            }),
        ];

        let mut orphaned = std::collections::HashSet::new();
        orphaned.insert("tool-1".to_string());

        remove_orphaned_tool_uses(&mut history, &orphaned);

        if let Message::Assistant(ref assistant_msg) = history[1] {
            assert!(
                assistant_msg.assistant_response_message.tool_uses.is_none(),
                "移除所有 tool_use 后应为 None"
            );
        } else {
            panic!("应该是 Assistant 消息");
        }
    }

    #[test]
    fn test_normalize_json_schema_coerces_field_types() {
        let input = serde_json::json!({
            "$schema": null,
            "type": null,
            "properties": null,
            "required": null,
            "additionalProperties": null,
        });

        let normalized = normalize_json_schema(input);

        assert_eq!(
            normalized.get("$schema").and_then(|v| v.as_str()),
            Some("http://json-schema.org/draft-07/schema#")
        );
        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("object")
        );
        assert!(normalized.get("properties").is_some_and(|v| v.is_object()));
        assert!(normalized.get("required").is_some_and(|v| v.is_array()));
        assert!(
            normalized
                .get("additionalProperties")
                .is_some_and(|v| v.is_boolean())
        );
    }

    #[test]
    fn test_image_wire_shape_matches_kiro_cli_format() {
        use crate::anthropic::types::Message as AnthropicMessage;
        let b64 = "iVBORw0KGgo=";
        for (media_type, expected_format) in &[
            ("image/png", "png"),
            ("image/jpeg", "jpeg"),
            ("image/gif", "gif"),
            ("image/webp", "webp"),
        ] {
            let req = MessagesRequest {
                model: "claude-sonnet-4".to_string(),
                max_tokens: 32,
                messages: vec![AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {
                            "type": "image",
                            "source": {"type": "base64", "media_type": media_type, "data": b64}
                        },
                        {"type": "text", "text": "describe"},
                    ]),
                }],
                stream: false,
                system: None,
                tools: None,
                tool_choice: None,
                thinking: None,
                output_config: None,
                reasoning_effort: None,
                metadata: None,
            };
            let result = convert_request(&req, &CompressionConfig::default(), None).unwrap();
            let imgs = &result
                .conversation_state
                .current_message
                .user_input_message
                .images;
            assert_eq!(
                imgs.len(),
                1,
                "media_type {} should yield 1 image",
                media_type
            );
            assert_eq!(
                imgs[0].format, *expected_format,
                "format mapping for {}",
                media_type
            );
            assert!(
                !imgs[0].source.bytes.is_empty(),
                "source.bytes must be populated"
            );
        }
    }

    #[test]
    fn test_normalize_json_schema_does_not_inject_missing_schema_field() {
        let input = serde_json::json!({
            "type": "object",
            "properties": {"x": {"type": "string"}},
        });
        let normalized = normalize_json_schema(input);
        assert!(
            normalized.get("$schema").is_none(),
            "missing $schema must NOT be auto-injected (wire alignment with kiro-cli)"
        );
        assert!(
            normalized.get("additionalProperties").is_none(),
            "missing additionalProperties must NOT be auto-injected"
        );
        assert_eq!(
            normalized.get("type").and_then(|v| v.as_str()),
            Some("object")
        );
        assert!(normalized.get("properties").is_some());
        assert!(normalized.get("required").is_some());
    }

    #[test]
    fn test_normalize_json_schema_filters_required_non_strings() {
        let input = serde_json::json!({
            "type": "object",
            "properties": {},
            "required": ["a", 1, null, {"x": 1}],
        });

        let normalized = normalize_json_schema(input);
        let required = normalized
            .get("required")
            .and_then(|v| v.as_array())
            .expect("required 应该是数组");

        assert_eq!(required, &vec![serde_json::Value::String("a".to_string())]);
    }

    #[test]
    fn test_chunked_policy_injected_only_with_write_edit_tools() {
        use crate::anthropic::types::{
            Message as AnthropicMessage, SystemMessage, Tool as AnthropicTool,
        };
        use std::collections::HashMap;

        let system = vec![SystemMessage {
            text: "You are a helpful assistant.".to_string(),
            block_type: None,
            cache_control: None,
        }];

        let req_no_tools = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hello"),
            }],
            stream: false,
            system: Some(system.clone()),
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req_no_tools, &CompressionConfig::default(), None).unwrap();
        let first_user = &result.conversation_state.history[0];
        match first_user {
            Message::User(u) => {
                assert!(
                    !u.user_input_message.content.contains("chunked operations"),
                    "无工具时不应注入 chunked policy"
                );
            }
            _ => panic!("history[0] 应该是 User 消息"),
        }

        let req_with_write = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hello"),
            }],
            stream: false,
            system: Some(system.clone()),
            tools: Some(vec![AnthropicTool {
                tool_type: None,
                name: "Write".to_string(),
                description: "Write a file".to_string(),
                input_schema: HashMap::new(),
                max_uses: None,
                cache_control: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req_with_write, &CompressionConfig::default(), None).unwrap();
        let first_user = &result.conversation_state.history[0];
        match first_user {
            Message::User(u) => {
                assert!(
                    u.user_input_message.content.contains("chunked operations"),
                    "有 Write 工具时应注入 chunked policy"
                );
            }
            _ => panic!("history[0] 应该是 User 消息"),
        }

        let req_no_system_with_edit = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hello"),
            }],
            stream: false,
            system: None,
            tools: Some(vec![AnthropicTool {
                tool_type: None,
                name: "Edit".to_string(),
                description: "Edit a file".to_string(),
                input_schema: HashMap::new(),
                max_uses: None,
                cache_control: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(
            &req_no_system_with_edit,
            &CompressionConfig::default(),
            None,
        )
        .unwrap();
        let first_user = &result.conversation_state.history[0];
        match first_user {
            Message::User(u) => {
                assert!(
                    u.user_input_message.content.contains("chunked operations"),
                    "system: None + 有 Edit 工具时也应注入 chunked policy"
                );
            }
            _ => panic!("history[0] 应该是 User 消息"),
        }
    }

    #[test]
    fn test_effort_whitelist_fallback() {
        use crate::anthropic::types::{Message as AnthropicMessage, OutputConfig, Thinking};

        // Opus 4.6 + adaptive → additionalModelRequestFields 有值
        let mut req = MessagesRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: Some(Thinking {
                thinking_type: "adaptive".to_string(),
                budget_tokens: 0,
            }),
            output_config: Some(OutputConfig {
                effort: "low".to_string(),
            }),
            reasoning_effort: None,
            metadata: None,
        };

        // thinking prefix 始终生成（与 additionalModelRequestFields 双通道）
        assert!(
            generate_thinking_prefix(&req).is_some(),
            "thinking prefix 应始终为 adaptive thinking 生成"
        );

        let fields = build_additional_model_request_fields(&req, None).unwrap();
        assert_eq!(fields["output_config"]["effort"], "low");

        req.output_config = Some(OutputConfig {
            effort: "ultra".to_string(),
        });
        let fields = build_additional_model_request_fields(&req, None).unwrap();
        assert_eq!(fields["output_config"]["effort"], "high");

        // 非 Opus 4.6 → additionalModelRequestFields 为 None，thinking 通过 XML 前缀注入
        req.model = "claude-sonnet-4-6".to_string();
        assert!(build_additional_model_request_fields(&req, None).is_none());
        assert!(generate_thinking_prefix(&req).is_some());
    }

    #[test]
    fn test_effort_normalization_respects_model_capabilities() {
        use crate::anthropic::types::{Message as AnthropicMessage, OutputConfig, Thinking};

        let mut req = MessagesRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: Some(Thinking {
                thinking_type: "adaptive".to_string(),
                budget_tokens: 0,
            }),
            output_config: Some(OutputConfig {
                effort: "xhigh".to_string(),
            }),
            reasoning_effort: None,
            metadata: None,
        };

        // Opus 4.6 clamps xhigh → high
        let fields = build_additional_model_request_fields(&req, None).unwrap();
        assert_eq!(fields["output_config"]["effort"], "high");

        // 非 Opus 4.6 模型不生成 additionalModelRequestFields
        req.model = "claude-opus-4-7".to_string();
        assert!(
            build_additional_model_request_fields(&req, None).is_none(),
            "非 Opus 4.6 模型不应生成 additionalModelRequestFields"
        );

        // Opus 4.6 MAX → max
        req.model = "claude-opus-4-6".to_string();
        req.output_config = Some(OutputConfig {
            effort: "  MAX  ".to_string(),
        });
        let fields = build_additional_model_request_fields(&req, None).unwrap();
        assert_eq!(fields["output_config"]["effort"], "max");
    }

    #[test]
    fn test_additional_model_request_fields_non_opus46_returns_none() {
        use crate::anthropic::types::{Message as AnthropicMessage, Thinking};

        // 非 Opus 4.6 模型 → additionalModelRequestFields 为 None（thinking 通过 XML 前缀）
        let req = MessagesRequest {
            model: "claude-sonnet-4-5-20250929".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: Some(Thinking {
                thinking_type: "adaptive".to_string(),
                budget_tokens: 0,
            }),
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        assert!(
            build_additional_model_request_fields(&req, None).is_none(),
            "非 Opus 4.6 模型不应生成 additionalModelRequestFields"
        );
        assert!(
            generate_thinking_prefix(&req).is_some(),
            "thinking 应通过 XML 前缀注入"
        );
    }

    #[test]
    fn test_additional_model_request_fields_opus46_budget_path() {
        use crate::anthropic::types::{Message as AnthropicMessage, OutputConfig, Thinking};

        // Opus 4.6 + adaptive → 有 additionalModelRequestFields
        let req = MessagesRequest {
            model: "claude-opus-4-6".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!("hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: Some(Thinking {
                thinking_type: "adaptive".to_string(),
                budget_tokens: 0,
            }),
            output_config: Some(OutputConfig {
                effort: "high".to_string(),
            }),
            reasoning_effort: None,
            metadata: None,
        };

        let fields = build_additional_model_request_fields(&req, None).unwrap();
        assert_eq!(fields["output_config"]["effort"], "high");

        // 非 adaptive（enabled）→ 不生成 additionalModelRequestFields
        let req_enabled = MessagesRequest {
            model: "claude-opus-4-6".to_string(),
            thinking: Some(Thinking {
                thinking_type: "enabled".to_string(),
                budget_tokens: 16_001,
            }),
            ..req.clone()
        };
        assert!(
            build_additional_model_request_fields(&req_enabled, None).is_none(),
            "非 adaptive thinking 不应生成 additionalModelRequestFields"
        );
    }

    #[test]
    fn test_extract_thinking_config_from_schema() {
        let schema = output_config_thinking_schema();
        let config = extract_thinking_config_from_schema(&schema).unwrap();
        assert_eq!(config.schema_path, ThinkingSchemaPath::OutputConfig);
        assert_eq!(config.efforts, ["low", "medium", "high", "xhigh", "max"]);

        let reasoning_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "reasoning": {
                    "type": "object",
                    "properties": {
                        "effort": { "enum": ["low", "high"] }
                    }
                }
            }
        });
        let config = extract_thinking_config_from_schema(&reasoning_schema).unwrap();
        assert_eq!(config.schema_path, ThinkingSchemaPath::Reasoning);
        assert_eq!(config.efforts, ["low", "high"]);
    }

    #[test]
    fn test_thinking_config_for_model_scopes_native_schema_to_claude_46_plus() {
        assert!(system::thinking_config_for_model("claude-sonnet-4-6").is_some());
        assert!(system::thinking_config_for_model("claude-opus-4.7-thinking").is_some());
        assert!(system::thinking_config_for_model("claude-sonnet-4-5-20250929").is_none());
        assert!(system::thinking_config_for_model("claude-haiku-4-5-20251001").is_none());
    }

    #[test]
    fn test_collect_history_tool_names_deduplicates_case_variants() {
        use crate::kiro::model::requests::tool::ToolUseEntry;

        let mut msg1 = AssistantMessage::new("reading...");
        msg1 = msg1.with_tool_uses(vec![
            ToolUseEntry::new("t-1", "read").with_input(serde_json::json!({"path": "/a.txt"})),
        ]);

        let mut msg2 = AssistantMessage::new("reading again...");
        msg2 = msg2.with_tool_uses(vec![
            ToolUseEntry::new("t-2", "Read").with_input(serde_json::json!({"path": "/b.txt"})),
        ]);

        let history = vec![
            Message::User(HistoryUserMessage::new("go", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: msg1,
            }),
            Message::User(HistoryUserMessage::new("ok", "claude-sonnet-4.5")),
            Message::Assistant(HistoryAssistantMessage {
                assistant_response_message: msg2,
            }),
        ];

        let tool_names = collect_history_tool_names(&history);
        assert_eq!(
            tool_names.len(),
            1,
            "大小写变体应被去重，实际: {:?}",
            tool_names
        );
        assert_eq!(tool_names[0], "read");
    }

    #[test]
    fn test_convert_request_handles_assistant_prefill() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("Hello"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!("Hi there"),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None);
        assert!(result.is_ok(), "prefill 场景不应报错: {:?}", result.err());
        let state = result.unwrap().conversation_state;
        assert_eq!(
            state.current_message.user_input_message.content, "Hello",
            "current_message 应为最后一条 user 消息的内容"
        );
    }

    #[test]
    fn test_convert_request_prefill_no_user_message() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "assistant".to_string(),
                content: serde_json::json!("Hi there"),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let err = convert_request(&req, &CompressionConfig::default(), None).unwrap_err();
        assert!(
            matches!(err, ConversionError::EmptyMessages),
            "只有 assistant 消息时应返回 EmptyMessages，实际: {:?}",
            err
        );
    }

    #[test]
    fn test_convert_request_empty_message_content() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!(""),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let err = convert_request(&req, &CompressionConfig::default(), None).unwrap_err();
        assert!(
            matches!(err, ConversionError::EmptyMessageContent),
            "空消息内容应返回 EmptyMessageContent，实际: {:?}",
            err
        );
    }

    #[test]
    fn test_convert_request_empty_text_block() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: serde_json::json!([
                    {"type": "text", "text": "   "},
                    {"type": "text", "text": "\n\t"}
                ]),
            }],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let err = convert_request(&req, &CompressionConfig::default(), None).unwrap_err();
        assert!(
            matches!(err, ConversionError::EmptyMessageContent),
            "仅包含空白文本的消息应返回 EmptyMessageContent，实际: {:?}",
            err
        );
    }

    #[test]
    fn test_convert_request_prefill_with_empty_user_message() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!(""),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!("Hi there"),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let err = convert_request(&req, &CompressionConfig::default(), None).unwrap_err();
        assert!(
            matches!(err, ConversionError::EmptyMessageContent),
            "prefill 回退后的空 user 消息应返回 EmptyMessageContent，实际: {:?}",
            err
        );
    }

    #[test]
    fn test_merge_consecutive_assistant_messages() {
        use crate::anthropic::types::Message as AnthropicMessage;
        use history::merge_assistant_messages;

        let msg1 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "thinking", "thinking": "Let me think about this..."},
                {"type": "text", "text": " "}
            ]),
        };

        let msg2 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "thinking", "thinking": "I should read the file."},
                {"type": "text", "text": "Let me read that file."},
                {"type": "tool_use", "id": "toolu_01ABC", "name": "read_file", "input": {"path": "/test.txt"}}
            ]),
        };

        let messages: Vec<&AnthropicMessage> = vec![&msg1, &msg2];
        let result = merge_assistant_messages(&messages, &mut HashMap::new()).expect("合并应成功");

        let content = &result.assistant_response_message.content;
        assert!(content.contains("<thinking>"), "应包含 thinking 标签");
        assert!(
            content.contains("Let me read that file"),
            "应包含第二条消息的 text 内容"
        );

        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("应有 tool_uses");
        assert_eq!(tool_uses.len(), 1);
        assert_eq!(tool_uses[0].tool_use_id, "toolu_01ABC");
        assert_eq!(tool_uses[0].name, "read_file");
    }

    #[test]
    fn test_consecutive_assistant_with_tool_use_result_pairing() {
        use crate::anthropic::types::Message as AnthropicMessage;

        let req = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 1024,
            messages: vec![
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!("Read the config file"),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "thinking", "thinking": "I need to read the file..."},
                        {"type": "text", "text": " "}
                    ]),
                },
                AnthropicMessage {
                    role: "assistant".to_string(),
                    content: serde_json::json!([
                        {"type": "thinking", "thinking": "Let me read the config."},
                        {"type": "text", "text": "I'll read the config file for you."},
                        {"type": "tool_use", "id": "toolu_01XYZ", "name": "read_file", "input": {"path": "/config.json"}}
                    ]),
                },
                AnthropicMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "tool_result", "tool_use_id": "toolu_01XYZ", "content": "{\"key\": \"value\"}"}
                    ]),
                },
            ],
            stream: false,
            system: None,
            tools: None,
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        };

        let result = convert_request(&req, &CompressionConfig::default(), None);
        assert!(
            result.is_ok(),
            "连续 assistant 消息场景不应报错: {:?}",
            result.err()
        );

        let state = result.unwrap().conversation_state;

        let mut found_tool_use = false;
        for msg in &state.history {
            if let Message::Assistant(assistant_msg) = msg
                && let Some(ref tool_uses) = assistant_msg.assistant_response_message.tool_uses
                && tool_uses.iter().any(|t| t.tool_use_id == "toolu_01XYZ")
            {
                found_tool_use = true;
                break;
            }
        }
        assert!(found_tool_use, "合并后的 assistant 消息应包含 tool_use");

        let tool_results = &state
            .current_message
            .user_input_message
            .user_input_message_context
            .tool_results;
        assert!(
            tool_results.iter().any(|t| t.tool_use_id == "toolu_01XYZ"),
            "current_message 应包含对应的 tool_result"
        );
    }

    #[test]
    fn test_merge_assistant_messages_multiple_tool_uses() {
        use crate::anthropic::types::Message as AnthropicMessage;
        use history::merge_assistant_messages;

        let msg1 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": "First action"},
                {"type": "tool_use", "id": "tool-1", "name": "read", "input": {"path": "/a.txt"}}
            ]),
        };

        let msg2 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": "Second action"},
                {"type": "tool_use", "id": "tool-2", "name": "write", "input": {"path": "/b.txt"}}
            ]),
        };

        let messages: Vec<&AnthropicMessage> = vec![&msg1, &msg2];
        let result = merge_assistant_messages(&messages, &mut HashMap::new()).expect("合并应成功");

        let tool_uses = result
            .assistant_response_message
            .tool_uses
            .expect("应有 tool_uses");

        assert_eq!(tool_uses.len(), 2, "应保留所有 tool_use");
        assert!(tool_uses.iter().any(|t| t.tool_use_id == "tool-1"));
        assert!(tool_uses.iter().any(|t| t.tool_use_id == "tool-2"));
    }

    #[test]
    fn test_merge_assistant_messages_only_tool_use() {
        use crate::anthropic::types::Message as AnthropicMessage;
        use history::merge_assistant_messages;

        let msg1 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "text", "text": " "}
            ]),
        };

        let msg2 = AnthropicMessage {
            role: "assistant".to_string(),
            content: serde_json::json!([
                {"type": "tool_use", "id": "tool-1", "name": "read", "input": {}}
            ]),
        };

        let messages: Vec<&AnthropicMessage> = vec![&msg1, &msg2];
        let result = merge_assistant_messages(&messages, &mut HashMap::new()).expect("合并应成功");

        assert!(
            result.assistant_response_message.content.is_empty(),
            "仅 tool_use 时合并阶段不应主动补 '.'"
        );
    }
}
