//! 历史消息构建与验证

use std::collections::HashMap;

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::kiro::model::requests::conversation::{
    AssistantMessage, HistoryAssistantMessage, HistoryUserMessage, Message,
    UserInputMessageContext, UserMessage,
};
use crate::kiro::model::requests::tool::{ToolResult, ToolUseEntry};
use crate::model::config::CompressionConfig;

use super::content::{non_empty_content_or_space, process_message_content};
use super::model::{
    ConversionError, KIRO_AGENTIC_SYSTEM_PROMPT, SYSTEM_ACK, SYSTEM_CHUNKED_POLICY,
};
use super::system::{generate_thinking_prefix, has_thinking_tags, wrap_system_for_history};
use super::tools::{has_write_or_edit_tool, map_tool_name};
use crate::anthropic::types::{ContentBlock, MessagesRequest};

/// 从 metadata.user_id 中提取 session UUID
///
/// 支持两种格式：
/// 1. 字符串格式: user_xxx_account__session_0b4445e1-f5be-49e1-87ce-62bbc28ad705
/// 2. JSON 格式: {"device_id":"...","account_uuid":"...","session_id":"UUID"}
///
/// 提取 session UUID 作为 conversationId
pub(super) fn extract_session_id(user_id: &str) -> Option<String> {
    // 先尝试 JSON 格式解析
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(user_id)
        && let Some(session_id) = json.get("session_id").and_then(|v| v.as_str())
        && is_valid_uuid(session_id)
    {
        return Some(session_id.to_string());
    }

    // 再尝试字符串格式：查找 "session_" 后面的内容
    if let Some(pos) = user_id.find("session_") {
        let session_part = &user_id[pos + 8..]; // "session_" 长度为 8
        // session_part 应该是 UUID 格式: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
        // 验证是否是有效的 UUID 格式（36 字符，包含 4 个连字符）
        if session_part.len() >= 36 {
            let uuid_str = &session_part[..36];
            if is_valid_uuid(uuid_str) {
                return Some(uuid_str.to_string());
            }
        }
    }
    None
}

/// 简单验证 UUID 格式（36 字符，包含 4 个连字符）
fn is_valid_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().filter(|c| *c == '-').count() == 4
}

/// 用**首条 user message** SHA-256 指纹派生跨 turn 稳定的 conversation_id。
///
/// **Round 7 重要更新（2026-05-13）**: 之前实现哈希 `messages[..len-1]`，但这个
/// 切片在每轮多轮里都在膨胀（turn N 切 [0..N-1]，turn N+1 切 [0..N+1]）
/// → SHA-256 必然不同 → conversation_id 每轮变。Round 7 cache 长程实测员
/// 抓了 8 轮 capture，conversationId 8 个不同值，**证实 fingerprint 每轮变**。
///
/// **但 cache 仍然命中**（同轮实测 13× credits/token 折扣）—— 因为 Bedrock prompt
/// cache 真实机制基于**请求 body 字节前缀哈希**（system + history 序列化），
/// **不依赖** conversationId。conversationId 在 Bedrock 看来只是上游遥测聚合
/// 字段，跨 turn 变化不影响 cache 命中。
///
/// 既然 cache 真因不在这里，fingerprint 的价值降级为"减少上游遥测 conv_id 噪声"。
/// 改为只哈希**首条 message** 的 role + content 文本：
///   - 跨 turn 字节级稳定（首条不变）
///   - 不含 thinking signature 这类客户端可能漂移的字段
///   - 不同会话有不同 fingerprint（首条 user 通常不同）
///
/// 返回 None: messages 空，让上层走 v4。
pub(super) fn messages_history_fingerprint(
    messages: &[crate::anthropic::types::Message],
) -> Option<String> {
    let first = messages.first()?;
    let mut hasher = Sha256::new();
    hasher.update(first.role.as_bytes());
    hasher.update(b"\x1f");
    // 优先取字符串形态（最常见、字节最稳定）；array 形态退而求其次走 JSON 序列化。
    // 注意：这里只哈希**首条** message —— Anthropic SDK 把 assistant 写回 history
    // 时不会回去改首条 user message，所以这是会话级稳定 anchor。
    if let Some(s) = first.content.as_str() {
        hasher.update(s.as_bytes());
    } else {
        let s = serde_json::to_string(&first.content).unwrap_or_default();
        hasher.update(s.as_bytes());
    }
    let digest = hasher.finalize();
    let bytes: [u8; 16] = digest[..16].try_into().ok()?;
    Some(Uuid::new_v5(&Uuid::NAMESPACE_DNS, &bytes).to_string())
}

/// 验证并过滤 tool_use/tool_result 配对
///
/// 收集所有 tool_use_id，验证 tool_result 是否匹配
/// 静默跳过孤立的 tool_use 和 tool_result，输出警告日志
///
/// # Arguments
/// * `history` - 历史消息引用
/// * `tool_results` - 当前消息中的 tool_result 列表
///
/// # Returns
/// 元组：(经过验证和过滤后的 tool_result 列表, 孤立的 tool_use_id 集合)
pub(super) fn validate_tool_pairing(
    history: &[Message],
    tool_results: &[ToolResult],
) -> (Vec<ToolResult>, std::collections::HashSet<String>) {
    use std::collections::HashSet;

    // 1. 收集所有历史中的 tool_use_id
    let mut all_tool_use_ids: HashSet<String> = HashSet::new();
    // 2. 收集历史中已经有 tool_result 的 tool_use_id
    let mut history_tool_result_ids: HashSet<String> = HashSet::new();

    for msg in history {
        match msg {
            Message::Assistant(assistant_msg) => {
                if let Some(ref tool_uses) = assistant_msg.assistant_response_message.tool_uses {
                    for tool_use in tool_uses {
                        all_tool_use_ids.insert(tool_use.tool_use_id.clone());
                    }
                }
            }
            Message::User(user_msg) => {
                // 收集历史 user 消息中的 tool_results
                for result in &user_msg
                    .user_input_message
                    .user_input_message_context
                    .tool_results
                {
                    history_tool_result_ids.insert(result.tool_use_id.clone());
                }
            }
        }
    }

    // 3. 计算真正未配对的 tool_use_ids（排除历史中已配对的）
    let mut unpaired_tool_use_ids: HashSet<String> = all_tool_use_ids
        .difference(&history_tool_result_ids)
        .cloned()
        .collect();

    // 4. 过滤并验证当前消息的 tool_results
    let mut filtered_results = Vec::new();

    for result in tool_results {
        if unpaired_tool_use_ids.contains(&result.tool_use_id) {
            // 配对成功
            filtered_results.push(result.clone());
            unpaired_tool_use_ids.remove(&result.tool_use_id);
        } else if all_tool_use_ids.contains(&result.tool_use_id) {
            // tool_use 存在但已经在历史中配对过了，这是重复的 tool_result
            tracing::warn!(
                "跳过重复的 tool_result：该 tool_use 已在历史中配对，tool_use_id={}",
                result.tool_use_id
            );
        } else {
            // 孤立 tool_result - 找不到对应的 tool_use
            tracing::warn!(
                "跳过孤立的 tool_result：找不到对应的 tool_use，tool_use_id={}",
                result.tool_use_id
            );
        }
    }

    // 5. 检测真正孤立的 tool_use（有 tool_use 但在历史和当前消息中都没有 tool_result）
    for orphaned_id in &unpaired_tool_use_ids {
        tracing::warn!(
            "检测到孤立的 tool_use：找不到对应的 tool_result，将从历史中移除，tool_use_id={}",
            orphaned_id
        );
    }

    // 配对验证汇总日志（仅在有异常时输出）
    if !unpaired_tool_use_ids.is_empty() || filtered_results.len() != tool_results.len() {
        tracing::warn!(
            total_tool_use_ids = all_tool_use_ids.len(),
            history_tool_result_ids = history_tool_result_ids.len(),
            orphaned_tool_use_count = unpaired_tool_use_ids.len(),
            orphaned_tool_use_ids = ?unpaired_tool_use_ids,
            input_tool_results = tool_results.len(),
            output_tool_results = filtered_results.len(),
            "tool_use/tool_result 配对验证完成（有异常）"
        );
    }

    (filtered_results, unpaired_tool_use_ids)
}

/// 从历史消息中移除孤立的 tool_use
///
/// Kiro API 要求每个 tool_use 必须有对应的 tool_result，否则返回 400 Bad Request。
/// 此函数遍历历史中的 assistant 消息，移除没有对应 tool_result 的 tool_use。
///
/// # Arguments
/// * `history` - 可变的历史消息列表
/// * `orphaned_ids` - 需要移除的孤立 tool_use_id 集合
pub(super) fn remove_orphaned_tool_uses(
    history: &mut [Message],
    orphaned_ids: &std::collections::HashSet<String>,
) {
    if orphaned_ids.is_empty() {
        return;
    }

    for msg in history.iter_mut() {
        if let Message::Assistant(assistant_msg) = msg
            && let Some(ref mut tool_uses) = assistant_msg.assistant_response_message.tool_uses
        {
            let original_len = tool_uses.len();
            tool_uses.retain(|tu| !orphaned_ids.contains(&tu.tool_use_id));

            // 如果移除后为空，设置为 None
            if tool_uses.is_empty() {
                assistant_msg.assistant_response_message.tool_uses = None;
            } else if tool_uses.len() != original_len {
                tracing::debug!(
                    "从 assistant 消息中移除了 {} 个孤立的 tool_use",
                    original_len - tool_uses.len()
                );
            }
        }
    }
}

pub(super) struct BuildHistoryContext<'a> {
    pub model_id: &'a str,
    pub compression_config: &'a CompressionConfig,
    pub total_image_count: usize,
    pub is_agentic: bool,
    pub remaining_image_budget: &'a mut usize,
    pub tool_name_map: &'a mut HashMap<String, String>,
}

/// 构建历史消息
/// `messages` 参数是经过 prefill 预处理后的消息切片（末尾必为 user）
pub(super) fn build_history(
    req: &MessagesRequest,
    messages: &[crate::anthropic::types::Message],
    ctx: BuildHistoryContext<'_>,
) -> Result<Vec<Message>, ConversionError> {
    let BuildHistoryContext {
        model_id,
        compression_config,
        total_image_count,
        is_agentic,
        remaining_image_budget,
        tool_name_map,
    } = ctx;
    let mut history = Vec::new();

    // 生成thinking前缀（如果需要）
    let thinking_prefix = generate_thinking_prefix(req);

    // 仅在请求包含 Write/Edit 工具时注入分块写入策略
    let should_inject_chunked_policy = has_write_or_edit_tool(req);

    // 1. 处理系统消息
    if let Some(ref system) = req.system {
        let system_content: String = system
            .iter()
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
            .join("\n");

        if !system_content.is_empty() {
            // 仅在存在 Write/Edit 工具时追加分块写入策略到系统消息
            let system_content = if should_inject_chunked_policy {
                format!("{}\n{}", system_content, SYSTEM_CHUNKED_POLICY)
            } else {
                system_content
            };

            // 系统消息作为 user + assistant 配对。
            //
            // kiro-cli 2.3.0 wire format (Q_LOG_LEVEL=trace 2026-05-12 capture):
            // system 注入是一个空的 CONTEXT ENTRY 包装 + 换行 + `Follow this
            // instruction: <text>`，对应的 assistant ack 是一段固定的长句。
            // 这段文案是上游 prefix-cache key 的稳定前缀，文案不一致会让每轮
            // 重算 system 部分，破坏 cache 命中。
            //
            // thinking_prefix（`<thinking_mode>...</thinking_mode>` 元控制标签）
            // 是给模型自己的元配置，不应当被包进 "Follow this instruction:"
            // 后面（语义错位 — 模型会把它当成用户指令的一部分）。把它放在
            // wrap 之外的最前面，与 system 指令分离。
            let wrapped_system = wrap_system_for_history(&system_content);
            let history_content = match (&thinking_prefix, has_thinking_tags(&system_content)) {
                (Some(prefix), false) => format!("{prefix}\n{wrapped_system}"),
                _ => wrapped_system,
            };
            let user_msg = HistoryUserMessage::new(history_content, model_id);
            history.push(Message::User(user_msg));

            let assistant_msg = HistoryAssistantMessage::new(SYSTEM_ACK);
            history.push(Message::Assistant(assistant_msg));
        }
    } else if thinking_prefix.is_some() || should_inject_chunked_policy {
        // 没有系统消息但需要注入 thinking 配置或分块写入策略。
        // 注：此分支没有用户原 system，仅注入控制标签；不走 CONTEXT ENTRY 包装
        // （CONTEXT ENTRY 是给"用户实际要求"用的，元控制标签不应混入）。
        let mut parts = Vec::new();
        if let Some(ref prefix) = thinking_prefix {
            parts.push(prefix.clone());
        }
        if should_inject_chunked_policy {
            // 分块写入策略是协议级行为指令；包成 Follow this instruction: 比裸
            // 注入更接近 kiro-cli 风格的多 system 处理。
            parts.push(wrap_system_for_history(SYSTEM_CHUNKED_POLICY));
        }
        let content = parts.join("\n");

        let user_msg = HistoryUserMessage::new(content, model_id);
        history.push(Message::User(user_msg));

        let assistant_msg = HistoryAssistantMessage::new(SYSTEM_ACK);
        history.push(Message::Assistant(assistant_msg));
    }

    // Agentic 模型：追加专用系统提示
    if is_agentic {
        let user_msg = HistoryUserMessage::new(KIRO_AGENTIC_SYSTEM_PROMPT, model_id);
        history.push(Message::User(user_msg));

        let assistant_msg =
            HistoryAssistantMessage::new("I will work autonomously following these principles.");
        history.push(Message::Assistant(assistant_msg));
    }

    // 2. 处理常规消息历史
    // 最后一条消息作为 currentMessage，不加入历史
    let history_end_index = messages.len().saturating_sub(1);

    // 收集并配对消息
    let mut user_buffer: Vec<&crate::anthropic::types::Message> = Vec::new();
    let mut assistant_buffer: Vec<&crate::anthropic::types::Message> = Vec::new();

    for msg in messages.iter().take(history_end_index) {
        if msg.role == "user" {
            // 先处理累积的 assistant 消息
            if !assistant_buffer.is_empty() {
                let merged = merge_assistant_messages(&assistant_buffer, tool_name_map)?;
                history.push(Message::Assistant(merged));
                assistant_buffer.clear();
            }
            user_buffer.push(msg);
        } else if msg.role == "assistant" {
            // 先处理累积的 user 消息
            if !user_buffer.is_empty() {
                let merged_user = merge_user_messages(
                    &user_buffer,
                    model_id,
                    compression_config,
                    total_image_count,
                    remaining_image_budget,
                )?;
                history.push(Message::User(merged_user));
                user_buffer.clear();
            }
            // 累积 assistant 消息（支持连续多条）
            assistant_buffer.push(msg);
        }
    }

    // 处理末尾累积的 assistant 消息
    if !assistant_buffer.is_empty() {
        let merged = merge_assistant_messages(&assistant_buffer, tool_name_map)?;
        history.push(Message::Assistant(merged));
    }

    // 处理结尾的孤立 user 消息
    if !user_buffer.is_empty() {
        let merged_user = merge_user_messages(
            &user_buffer,
            model_id,
            compression_config,
            total_image_count,
            remaining_image_budget,
        )?;
        history.push(Message::User(merged_user));

        // 自动配对一个 "OK" 的 assistant 响应
        let auto_assistant = HistoryAssistantMessage::new("OK");
        history.push(Message::Assistant(auto_assistant));
    }

    // 历史结构摘要日志
    {
        let mut user_count = 0usize;
        let mut assistant_count = 0usize;
        let mut synthetic_count = 0usize;
        let mut image_count_in_history = 0usize;

        // system 消息产生的合成配对
        if req
            .system
            .as_ref()
            .is_some_and(|s| s.iter().any(|b| !b.text.is_empty()))
        {
            synthetic_count += 2;
        }

        for msg in &history {
            match msg {
                Message::User(u) => {
                    user_count += 1;
                    image_count_in_history += u.user_input_message.images.len();
                }
                Message::Assistant(_) => assistant_count += 1,
            }
        }

        tracing::info!(
            history_len = history.len(),
            user_messages = user_count,
            assistant_messages = assistant_count,
            synthetic_pairs = synthetic_count,
            images_in_history = image_count_in_history,
            "历史消息结构摘要"
        );
    }

    Ok(history)
}

fn merge_user_messages(
    messages: &[&crate::anthropic::types::Message],
    model_id: &str,
    compression_config: &CompressionConfig,
    total_image_count: usize,
    remaining_image_budget: &mut usize,
) -> Result<HistoryUserMessage, ConversionError> {
    let mut content_parts = Vec::new();
    let mut all_images = Vec::new();
    let mut all_tool_results = Vec::new();

    for msg in messages {
        let (text, images, tool_results) = process_message_content(
            &msg.content,
            compression_config,
            total_image_count,
            remaining_image_budget,
        )?;
        if !text.is_empty() {
            content_parts.push(text);
        }
        all_images.extend(images);
        all_tool_results.extend(tool_results);
    }

    let content = non_empty_content_or_space(
        content_parts.join("\n"),
        !all_images.is_empty() || !all_tool_results.is_empty(),
    );
    // 历史 user 消息尽量避免主动补 "."：
    // 若只有非文本载荷，则保留空字符串让真实结构(images/tool_results)表达语义；
    // 仅纯文本且被压成空白时才做最终兜底，避免空 content 直发下游。
    let content =
        if content.trim().is_empty() && all_images.is_empty() && all_tool_results.is_empty() {
            ".".to_string()
        } else {
            content
        };
    // 保留文本内容，即使有工具结果也不丢弃用户文本
    let mut user_msg = UserMessage::new(&content, model_id);

    if !all_images.is_empty() {
        user_msg = user_msg.with_images(all_images);
    }

    if !all_tool_results.is_empty() {
        let mut ctx = UserInputMessageContext::new();
        ctx = ctx.with_tool_results(all_tool_results);
        user_msg = user_msg.with_context(ctx);
    }

    Ok(HistoryUserMessage {
        user_input_message: user_msg,
    })
}

/// 转换 assistant 消息
pub(super) fn convert_assistant_message(
    msg: &crate::anthropic::types::Message,
    tool_name_map: &mut HashMap<String, String>,
) -> Result<HistoryAssistantMessage, ConversionError> {
    let mut thinking_content = String::new();
    let mut text_content = String::new();
    let mut tool_uses = Vec::new();

    match &msg.content {
        serde_json::Value::String(s) => {
            text_content = s.clone();
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "thinking" => {
                            if let Some(thinking) = block.thinking {
                                thinking_content.push_str(&thinking);
                            }
                        }
                        "text" => {
                            if let Some(text) = block.text {
                                text_content.push_str(&text);
                            }
                        }
                        "tool_use" => {
                            if let (Some(id), Some(name)) = (block.id, block.name) {
                                let input = block.input.unwrap_or(serde_json::json!({}));
                                let mapped_name = map_tool_name(&name, tool_name_map);
                                tool_uses
                                    .push(ToolUseEntry::new(id, mapped_name).with_input(input));
                            }
                        }
                        // WebSearch 相关块：server_tool_use 忽略，
                        // web_search_tool_result 提取 title/url/snippet/page_age 保留到对话历史
                        "server_tool_use" => {}
                        "web_search_tool_result" => {
                            // 注意：历史文本长度由 compressor.rs 的压缩流程统一管理，
                            // 此处无需额外截断。实际效果需运行时验证。
                            //
                            // encrypted_content 字段：本项目中由 websearch.rs 写入，
                            // 存储的是可读 snippet；若上游改为原生 Anthropic WebSearch 响应，
                            // 该字段语义可能不同，届时需重新评估是否保留。
                            if let Some(serde_json::Value::Array(results)) = block.content {
                                for result in &results {
                                    if result.get("type").and_then(|t| t.as_str())
                                        == Some("web_search_result")
                                    {
                                        let title = result
                                            .get("title")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or("");
                                        let url = result
                                            .get("url")
                                            .and_then(|u| u.as_str())
                                            .unwrap_or("");
                                        if !url.is_empty() {
                                            // 历史上下文仅供 Kiro 模型理解，使用纯文本格式
                                            // 避免 title/url 含特殊字符时 Markdown 语法被破坏
                                            let snippet = result
                                                .get("encrypted_content")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("");
                                            let page_age = result
                                                .get("page_age")
                                                .and_then(|s| s.as_str())
                                                .unwrap_or("");
                                            // 剥除控制字符（换行等），保持单行
                                            let clean_title: String =
                                                title.chars().filter(|c| !c.is_control()).collect();
                                            let clean_snippet: String = snippet
                                                .chars()
                                                .filter(|c| !c.is_control())
                                                .collect();
                                            if clean_title.is_empty() {
                                                text_content.push_str(&format!("{}\n", url));
                                            } else {
                                                text_content.push_str(&format!(
                                                    "{}: {}\n",
                                                    clean_title, url
                                                ));
                                            }
                                            if !page_age.is_empty() {
                                                text_content
                                                    .push_str(&format!("Date: {}\n", page_age));
                                            }
                                            if !clean_snippet.is_empty() {
                                                text_content
                                                    .push_str(&format!("{}\n", clean_snippet));
                                            }
                                            text_content.push('\n');
                                        }
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    // 组合 thinking 和 text 内容
    // 格式: <thinking>思考内容</thinking>\n\ntext内容
    let final_content = if !thinking_content.is_empty() {
        if !text_content.is_empty() {
            format!(
                "<thinking>{}</thinking>\n\n{}",
                thinking_content, text_content
            )
        } else {
            format!("<thinking>{}</thinking>", thinking_content)
        }
    } else {
        text_content
    };

    let mut assistant = AssistantMessage::new(final_content);
    if !tool_uses.is_empty() {
        // kiro-cli 2.3.0 wire: every toolUse turn carries a `messageId` UUID
        // (gar-body-3.json line 38). Plain-text turns omit it. We mint a fresh
        // UUID — server-side it appears to be opaque/ID-only.
        assistant.message_id = Some(Uuid::new_v4().to_string());
        assistant = assistant.with_tool_uses(tool_uses);
    }

    Ok(HistoryAssistantMessage {
        assistant_response_message: assistant,
    })
}

/// 合并多个连续的 assistant 消息为一条
/// 用于处理网络不稳定时产生的连续 assistant 消息（Issue #79）
pub(super) fn merge_assistant_messages(
    messages: &[&crate::anthropic::types::Message],
    tool_name_map: &mut HashMap<String, String>,
) -> Result<HistoryAssistantMessage, ConversionError> {
    assert!(!messages.is_empty());
    if messages.len() == 1 {
        return convert_assistant_message(messages[0], tool_name_map);
    }

    let mut all_tool_uses: Vec<ToolUseEntry> = Vec::new();
    let mut content_parts: Vec<String> = Vec::new();

    for msg in messages {
        let converted = convert_assistant_message(msg, tool_name_map)?;
        let am = converted.assistant_response_message;
        if !am.content.trim().is_empty() {
            content_parts.push(am.content);
        }
        if let Some(tus) = am.tool_uses {
            all_tool_uses.extend(tus);
        }
    }

    let content = if content_parts.is_empty() {
        String::new()
    } else {
        content_parts.join("\n\n")
    };

    let mut assistant = AssistantMessage::new(content);
    if !all_tool_uses.is_empty() {
        assistant.message_id = Some(Uuid::new_v4().to_string());
        assistant = assistant.with_tool_uses(all_tool_uses);
    }
    Ok(HistoryAssistantMessage {
        assistant_response_message: assistant,
    })
}
