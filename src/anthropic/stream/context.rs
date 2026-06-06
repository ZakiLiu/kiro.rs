//! 流处理上下文

use std::collections::HashMap;

use serde_json::json;
use uuid::Uuid;

use crate::anthropic::error_map::{self, ErrorCategory, ErrorRequestContext};
use crate::common::utf8::floor_char_boundary;
use crate::kiro::model::events::{Event, MeteringEvent, TokenUsageEvent};

use super::event::SseEvent;
use super::state::SseStateManager;
use super::thinking::{
    find_real_thinking_end_tag, find_real_thinking_end_tag_at_buffer_end,
    find_real_thinking_start_tag,
};
use super::usage::{CacheUsageBreakdown, FinalUsage, billed_input_tokens, estimate_tokens};
/// 流处理上下文
pub struct StreamContext {
    /// SSE 状态管理器
    pub state_manager: SseStateManager,
    /// 请求的模型名称
    pub model: String,
    /// 消息 ID
    pub message_id: String,
    /// 输入 tokens（估算值）
    pub input_tokens: i32,
    /// cache usage 统计（可选）
    pub cache_usage: Option<CacheUsageBreakdown>,
    /// 从 contextUsageEvent 计算的实际输入 tokens
    pub context_input_tokens: Option<i32>,
    /// 输出 tokens 累计（不含 thinking）
    pub output_tokens: i32,
    /// thinking tokens 累计（独立计数，不计入 output_tokens）
    pub thinking_tokens: i32,
    /// 工具块索引映射 (tool_id -> block_index)
    pub tool_block_indices: HashMap<String, i32>,
    /// 工具名称反向映射（短名称 → 原始名称），用于响应时还原
    pub tool_name_map: HashMap<String, String>,
    /// thinking 是否启用
    pub thinking_enabled: bool,
    /// thinking 内容缓冲区
    pub thinking_buffer: String,
    /// 是否在 thinking 块内
    pub in_thinking_block: bool,
    /// thinking 块是否已提取完成
    pub thinking_extracted: bool,
    /// thinking 块索引
    pub thinking_block_index: Option<i32>,
    /// Q 上游 reasoningContentEvent 通过独立事件流推 thinking 内容（与老的
    /// 嵌入 `<thinking>` 标签格式互斥）；此字段标记 reasoning 块是否打开。
    pub reasoning_block_open: bool,
    /// reasoningContentEvent 末尾 payload 可能带 signature，需要在 content_block_stop
    /// 之前 emit signature_delta；缓存它直到关闭 thinking 块的时机。
    pub pending_reasoning_signature: Option<String>,
    /// 文本块索引（按需动态分配）
    pub text_block_index: Option<i32>,
    /// 上游 meteringEvent 透传的 credit usage，仅用于最终 usage 统计，不生成独立 SSE 事件
    pub metering: Option<MeteringEvent>,
    /// 上游 tokenUsageEvent 精确计量（流末端下发），有值时覆盖本地估算
    pub token_usage: Option<TokenUsageEvent>,
    /// 是否需要剥离 thinking 内容开头的换行符
    /// 模型输出 `<thinking>\n` 时，`\n` 可能与标签在同一 chunk 或下一 chunk
    strip_thinking_leading_newline: bool,
}

impl StreamContext {
    /// 创建启用thinking的StreamContext
    pub fn new_with_thinking(
        model: impl Into<String>,
        input_tokens: i32,
        cache_usage: Option<CacheUsageBreakdown>,
        thinking_enabled: bool,
        tool_name_map: HashMap<String, String>,
    ) -> Self {
        Self {
            state_manager: SseStateManager::new(),
            model: model.into(),
            message_id: format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
            input_tokens,
            cache_usage,
            context_input_tokens: None,
            output_tokens: 0,
            thinking_tokens: 0,
            tool_block_indices: HashMap::new(),
            tool_name_map,
            thinking_enabled,
            thinking_buffer: String::new(),
            in_thinking_block: false,
            thinking_extracted: false,
            thinking_block_index: None,
            reasoning_block_open: false,
            pending_reasoning_signature: None,
            text_block_index: None,
            metering: None,
            token_usage: None,
            strip_thinking_leading_newline: false,
        }
    }

    /// 生成 message_start 事件
    pub fn create_message_start_event(&self) -> serde_json::Value {
        let billed_input_tokens = self
            .cache_usage
            .map(|cache_usage| {
                billed_input_tokens(
                    self.input_tokens,
                    cache_usage.cache_creation_input_tokens,
                    cache_usage.cache_read_input_tokens,
                )
            })
            .unwrap_or(self.input_tokens);
        let mut usage = json!({
            "input_tokens": billed_input_tokens,
            "output_tokens": 1,
        });
        if let Some(cache_usage) = self.cache_usage {
            usage["cache_creation_input_tokens"] = json!(cache_usage.cache_creation_input_tokens);
            usage["cache_read_input_tokens"] = json!(cache_usage.cache_read_input_tokens);
        }
        json!({
            "type": "message_start",
            "message": {
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": self.model,
                "stop_reason": null,
                "stop_sequence": null,
                "usage": usage
            }
        })
    }

    /// 生成初始事件序列（仅 message_start）
    ///
    /// 注意：不再在初始化阶段创建空 text block。
    /// 否则当模型首个输出为 tool_use（且没有任何 text_delta）时，
    /// 会产生一个空的 text content block（text=""），客户端写回 history 后会触发上游校验拒绝。
    pub fn generate_initial_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // message_start
        let msg_start = self.create_message_start_event();
        if let Some(event) = self.state_manager.handle_message_start(msg_start) {
            events.push(event);
        }

        events
    }

    /// 处理 Kiro 事件并转换为 Anthropic SSE 事件
    pub fn process_kiro_event(&mut self, event: &Event) -> Vec<SseEvent> {
        match event {
            Event::InitialResponse { conversation_id } => {
                // 服务端首帧含 server-authoritative conversationId。当前
                // 我们已经用客户端派生 ID（converter.rs::convert_request），
                // 这里只用作可观测性 — 把上游 ID 落到 trace 便于关联日志。
                if !conversation_id.is_empty() {
                    tracing::debug!(
                        server_conversation_id = %conversation_id,
                        "收到 initialResponseEvent"
                    );
                }
                Vec::new()
            }
            Event::ReasoningContent(reasoning) => self.process_reasoning_content(reasoning),
            Event::AssistantResponse(resp) => {
                // 切换到 text 流时关闭 reasoning 块（先 signature_delta 再 stop）
                let mut events = self.close_reasoning_if_open();
                events.extend(self.process_assistant_response(&resp.content));
                events
            }
            Event::ToolUse(tool_use) => {
                let mut events = self.close_reasoning_if_open();
                events.extend(self.process_tool_use(tool_use));
                events
            }
            Event::ContextUsage(context_usage) => {
                // 从上下文使用百分比计算实际的 input_tokens
                let context_window = crate::anthropic::types::get_context_window_size(&self.model) as f64;
                let actual_input_tokens =
                    (context_usage.context_usage_percentage * context_window / 100.0) as i32;
                self.context_input_tokens = Some(actual_input_tokens);
                // 上下文使用量达到 100% 时，设置 stop_reason 为 model_context_window_exceeded
                if context_usage.context_usage_percentage >= 100.0 {
                    self.state_manager
                        .set_stop_reason("model_context_window_exceeded");
                }
                tracing::debug!(
                    "收到 contextUsageEvent: {:.4}%, 计算 input_tokens: {} (context_window: {})",
                    context_usage.context_usage_percentage,
                    actual_input_tokens,
                    context_window as i32
                );
                Vec::new()
            }
            Event::Metering(metering) => {
                self.metering = Some(metering.clone());
                tracing::debug!(
                    usage = metering.usage,
                    unit = %metering.unit,
                    unit_plural = %metering.unit_plural,
                    "收到 meteringEvent"
                );
                Vec::new()
            }
            Event::TokenUsage(token_usage) => {
                if token_usage.has_real_usage() {
                    self.token_usage = Some(token_usage.clone());
                    tracing::info!(
                        uncached_input = token_usage.uncached_input_tokens,
                        output = token_usage.output_tokens,
                        total = token_usage.total_tokens,
                        cache_read = ?token_usage.cache_read_input_tokens,
                        cache_write = ?token_usage.cache_write_input_tokens,
                        "收到 tokenUsageEvent — 使用上游精确值替代本地估算"
                    );
                }
                Vec::new()
            }
            Event::Error {
                error_code,
                error_message,
            } => {
                // 使用 ErrorCategory 结构化分类记录流内错误（用于可观测性）
                let err_for_classify = anyhow::anyhow!("{}: {}", error_code, error_message);
                let ctx = ErrorRequestContext::default();
                let category: ErrorCategory = error_map::classify(&err_for_classify, &ctx);
                tracing::error!(
                    error_code = %error_code,
                    error_message = %error_message,
                    error_category = ?category,
                    "收到流内错误事件"
                );
                // 上游业务错误 → 转 Anthropic SSE error event + 标记 stop_reason
                // 让客户端能感知（throttling、content filter 等）。
                self.state_manager.set_stop_reason("error");
                vec![SseEvent::new(
                    "error",
                    json!({
                        "type": "error",
                        "error": {
                            "type": error_code,
                            "message": error_message,
                        }
                    }),
                )]
            }
            Event::Exception {
                exception_type,
                message,
            } => {
                // 处理 ContentLengthExceededException
                if exception_type == "ContentLengthExceededException" {
                    self.state_manager.set_stop_reason("max_tokens");
                    tracing::warn!("收到异常事件: {} - {}", exception_type, message);
                    return Vec::new();
                }
                tracing::warn!("收到异常事件: {} - {}", exception_type, message);
                // 其他 exception → 也转 SSE error event 并标记 stop_reason=error
                self.state_manager.set_stop_reason("error");
                vec![SseEvent::new(
                    "error",
                    json!({
                        "type": "error",
                        "error": {
                            "type": exception_type,
                            "message": message,
                        }
                    }),
                )]
            }
            _ => Vec::new(),
        }
    }

    /// 处理 reasoningContentEvent — Q 上游对 thinking 模型推送的独立推理流。
    ///
    /// 与老式嵌入 `<thinking>` 标签的 assistantResponseEvent 路径**互斥**：
    /// 实际生产中只会走一条路径（新模型走 reasoningContentEvent，旧 assistant
    /// 文本流走 process_content_with_thinking）。
    ///
    /// Anthropic SSE 协议要求：
    ///   content_block_start (thinking) → thinking_delta* → signature_delta → content_block_stop
    fn process_reasoning_content(
        &mut self,
        reasoning: &crate::kiro::model::events::ReasoningContentEvent,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 首次进入 reasoning：分配块索引、emit content_block_start
        if !self.reasoning_block_open {
            let index = self.state_manager.next_block_index();
            self.thinking_block_index = Some(index);
            self.reasoning_block_open = true;
            events.extend(self.state_manager.handle_content_block_start(
                index,
                "thinking",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": { "type": "thinking", "thinking": "" }
                }),
            ));
        }

        let Some(index) = self.thinking_block_index else {
            return events;
        };

        if !reasoning.text.is_empty() {
            self.thinking_tokens += estimate_tokens(&reasoning.text);
            if let Some(delta_event) = self
                .state_manager
                .handle_content_block_delta(
                    index,
                    json!({
                        "type": "content_block_delta",
                        "index": index,
                        "delta": { "type": "thinking_delta", "thinking": reasoning.text }
                    }),
                )
            {
                events.push(delta_event);
            }
        }

        // 暂存 signature，在关闭块时一并发出（Anthropic 规范要求 signature_delta
        // 紧接在 content_block_stop 之前）
        if let Some(sig) = &reasoning.signature {
            self.pending_reasoning_signature = Some(sig.clone());
        }

        events
    }

    /// 切换到 text/tool_use 流之前关闭 reasoning 块：
    /// emit signature_delta（若 pending）→ content_block_stop。
    fn close_reasoning_if_open(&mut self) -> Vec<SseEvent> {
        if !self.reasoning_block_open {
            return Vec::new();
        }
        let Some(index) = self.thinking_block_index else {
            self.reasoning_block_open = false;
            return Vec::new();
        };

        let mut events = Vec::new();

        // signature_delta：优先用上游 reasoning event 真实 signature。
        // Anthropic 规范要求字段存在，没有就发空串：
        //   - Round 6 实测：客户端 SDK 接受空串、Kiro 上游接受 200
        //   - 但下一轮 history 回写时上游可能拒（"thinking-cache token 不合法"）
        //   - 真实 reasoningContentEvent 通常带 signature，若未带 → log 警告便于追踪
        let signature = match self.pending_reasoning_signature.take() {
            Some(sig) if !sig.is_empty() => sig,
            _ => {
                tracing::warn!(
                    "reasoning 块关闭时无 signature（上游未在 reasoningContentEvent 中提供），\
                     使用空串占位。下一轮 history 回写该 thinking 块可能被上游拒。"
                );
                String::new()
            }
        };
        if let Some(delta_event) = self.state_manager.handle_content_block_delta(
            index,
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": { "type": "signature_delta", "signature": signature }
            }),
        ) {
            events.push(delta_event);
        }

        if let Some(stop_event) = self.state_manager.handle_content_block_stop(index) {
            events.push(stop_event);
        }
        self.reasoning_block_open = false;
        // 清掉 thinking_block_index，避免老路径 process_content_with_thinking 误用同一索引
        // （Anthropic 协议允许多 thinking 块，但 index 必须不同；不清空可能让两条路径共用 index）
        self.thinking_block_index = None;
        events
    }

    /// 处理助手响应事件
    pub(super) fn process_assistant_response(&mut self, content: &str) -> Vec<SseEvent> {
        if content.is_empty() {
            return Vec::new();
        }

        // 估算 tokens
        self.output_tokens += estimate_tokens(content);

        // 如果启用了thinking，需要处理thinking块
        if self.thinking_enabled {
            return self.process_content_with_thinking(content);
        }

        // 非 thinking 模式同样复用统一的 text_delta 发送逻辑，
        // 以便在 tool_use 自动关闭文本块后能够自愈重建新的文本块，避免“吞字”。
        self.create_text_delta_events(content)
    }

    /// 处理包含thinking块的内容
    fn process_content_with_thinking(&mut self, content: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 将内容添加到缓冲区进行处理
        self.thinking_buffer.push_str(content);

        loop {
            if !self.in_thinking_block && !self.thinking_extracted {
                // 查找 <thinking> 开始标签（跳过被反引号包裹的）
                if let Some(start_pos) = find_real_thinking_start_tag(&self.thinking_buffer) {
                    // 发送 <thinking> 之前的内容作为 text_delta
                    // 注意：如果前面只是空白字符（如 adaptive 模式返回的 \n\n），则跳过，
                    // 避免在 thinking 块之前产生无意义的 text 块导致客户端解析失败
                    let before_thinking = self.thinking_buffer[..start_pos].to_string();
                    if !before_thinking.is_empty() && !before_thinking.trim().is_empty() {
                        events.extend(self.create_text_delta_events(&before_thinking));
                    }

                    // 进入 thinking 块
                    self.in_thinking_block = true;
                    self.strip_thinking_leading_newline = true;
                    self.thinking_buffer =
                        self.thinking_buffer[start_pos + "<thinking>".len()..].to_string();

                    // 创建 thinking 块的 content_block_start 事件
                    let thinking_index = self.state_manager.next_block_index();
                    self.thinking_block_index = Some(thinking_index);
                    let start_events = self.state_manager.handle_content_block_start(
                        thinking_index,
                        "thinking",
                        json!({
                            "type": "content_block_start",
                            "index": thinking_index,
                            "content_block": {
                                "type": "thinking",
                                "thinking": ""
                            }
                        }),
                    );
                    events.extend(start_events);
                } else {
                    // 没有找到 <thinking>，检查是否可能是部分标签
                    // 保留可能是部分标签的内容
                    let target_len = self
                        .thinking_buffer
                        .len()
                        .saturating_sub("<thinking>".len());
                    let safe_len = floor_char_boundary(&self.thinking_buffer, target_len);
                    if safe_len > 0 {
                        let safe_content = self.thinking_buffer[..safe_len].to_string();
                        // 如果 thinking 尚未提取，且安全内容只是空白字符，
                        // 则不发送为 text_delta，继续保留在缓冲区等待更多内容。
                        // 这避免了 4.6 模型中 <thinking> 标签跨事件分割时，
                        // 前导空白（如 "\n\n"）被错误地创建为 text 块，
                        // 导致 text 块先于 thinking 块出现的问题。
                        if !safe_content.is_empty() && !safe_content.trim().is_empty() {
                            events.extend(self.create_text_delta_events(&safe_content));
                            self.thinking_buffer = self.thinking_buffer[safe_len..].to_string();
                        }
                    }
                    break;
                }
            } else if self.in_thinking_block {
                // 剥离 <thinking> 标签后紧跟的换行符（可能跨 chunk）
                if self.strip_thinking_leading_newline {
                    if self.thinking_buffer.starts_with('\n') {
                        self.thinking_buffer = self.thinking_buffer[1..].to_string();
                        self.strip_thinking_leading_newline = false;
                    } else if !self.thinking_buffer.is_empty() {
                        // buffer 非空但不以 \n 开头，不再需要剥离
                        self.strip_thinking_leading_newline = false;
                    }
                    // buffer 为空时保留标志，等待下一个 chunk
                }

                // 在 thinking 块内，查找 </thinking> 结束标签（跳过被反引号包裹的）
                if let Some(end_pos) = find_real_thinking_end_tag(&self.thinking_buffer) {
                    // 提取 thinking 内容
                    let thinking_content = self.thinking_buffer[..end_pos].to_string();
                    if let Some(thinking_index) = self.thinking_block_index
                        && !thinking_content.is_empty()
                    {
                        events.push(
                            self.create_thinking_delta_event(thinking_index, &thinking_content),
                        );
                    }

                    // 结束 thinking 块
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;

                    // 发送空的 thinking_delta 事件，然后发送 content_block_stop 事件
                    if let Some(thinking_index) = self.thinking_block_index {
                        // 先发送空的 thinking_delta
                        events.push(self.create_thinking_delta_event(thinking_index, ""));
                        // 在 content_block_stop 之前发 signature_delta（Anthropic 规范）
                        events.push(self.create_thinking_signature_event(thinking_index));
                        // 再发送 content_block_stop
                        if let Some(stop_event) =
                            self.state_manager.handle_content_block_stop(thinking_index)
                        {
                            events.push(stop_event);
                        }
                    }

                    // 剥离 `</thinking>\n\n`（find_real_thinking_end_tag 已确认 \n\n 存在）
                    self.thinking_buffer =
                        self.thinking_buffer[end_pos + "</thinking>\n\n".len()..].to_string();
                } else {
                    // 没有找到结束标签，发送当前缓冲区内容作为 thinking_delta。
                    // 保留末尾可能是部分 `</thinking>\n\n` 的内容：
                    // find_real_thinking_end_tag 要求标签后有 `\n\n` 才返回 Some，
                    // 因此保留区必须覆盖 `</thinking>\n\n` 的完整长度（13 字节），
                    // 否则当 `</thinking>` 已在 buffer 但 `\n\n` 尚未到达时，
                    // 标签的前几个字符会被错误地作为 thinking_delta 发出。
                    let target_len = self
                        .thinking_buffer
                        .len()
                        .saturating_sub("</thinking>\n\n".len());
                    let safe_len = floor_char_boundary(&self.thinking_buffer, target_len);
                    if safe_len > 0 {
                        let safe_content = self.thinking_buffer[..safe_len].to_string();
                        if let Some(thinking_index) = self.thinking_block_index
                            && !safe_content.is_empty()
                        {
                            events.push(
                                self.create_thinking_delta_event(thinking_index, &safe_content),
                            );
                        }
                        self.thinking_buffer = self.thinking_buffer[safe_len..].to_string();
                    }
                    break;
                }
            } else {
                // thinking 已提取完成，剩余内容作为 text_delta
                if !self.thinking_buffer.is_empty() {
                    let remaining = self.thinking_buffer.clone();
                    self.thinking_buffer.clear();
                    events.extend(self.create_text_delta_events(&remaining));
                }
                break;
            }
        }

        events
    }

    /// 创建 text_delta 事件
    ///
    /// 如果文本块尚未创建，会先创建文本块。
    /// 当发生 tool_use 时，状态机会自动关闭当前文本块；后续文本会自动创建新的文本块继续输出。
    ///
    /// 返回值包含可能的 content_block_start 事件和 content_block_delta 事件。
    fn create_text_delta_events(&mut self, text: &str) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 如果当前 text_block_index 指向的块已经被关闭（例如 tool_use 开始时自动 stop），
        // 则丢弃该索引并创建新的文本块继续输出，避免 delta 被状态机拒绝导致"吞字"。
        if let Some(idx) = self.text_block_index
            && !self.state_manager.is_block_open_of_type(idx, "text")
        {
            self.text_block_index = None;
        }

        // 获取或创建文本块索引
        let text_index = if let Some(idx) = self.text_block_index {
            idx
        } else {
            // 文本块尚未创建，需要先创建
            let idx = self.state_manager.next_block_index();
            self.text_block_index = Some(idx);

            // 发送 content_block_start 事件
            let start_events = self.state_manager.handle_content_block_start(
                idx,
                "text",
                json!({
                    "type": "content_block_start",
                    "index": idx,
                    "content_block": {
                        "type": "text",
                        "text": ""
                    }
                }),
            );
            events.extend(start_events);
            idx
        };

        // 发送 content_block_delta 事件
        if let Some(delta_event) = self.state_manager.handle_content_block_delta(
            text_index,
            json!({
                "type": "content_block_delta",
                "index": text_index,
                "delta": {
                    "type": "text_delta",
                    "text": text
                }
            }),
        ) {
            events.push(delta_event);
        }

        events
    }

    /// 创建 thinking_delta 事件
    fn create_thinking_delta_event(&self, index: i32, thinking: &str) -> SseEvent {
        SseEvent::new(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {
                    "type": "thinking_delta",
                    "thinking": thinking
                }
            }),
        )
    }

    /// 创建 signature_delta 事件（thinking 块关闭前发送）。
    ///
    /// Anthropic Messages 规范要求 `thinking` content block 在 `content_block_stop`
    /// 之前发一个 `signature_delta`，客户端 SDK 会把它原样写回 history。
    ///
    /// **Round 6 实测结论**（2026-05-13）：
    /// - 空字符串 `""`：Anthropic SDK 接受、Kiro 上游接受（HTTP 200）、客户端 schema 校验通过
    /// - 缺字段：上游 HTTP 502 — 所以字段**必须存在**
    /// - 任意伪造签名（含 SHA-256(thinking) / stub 占位）：Kiro 上游不识别为合法
    ///   thinking-cache token，`cache_read_input_tokens` 仍为 0。
    ///
    /// 因此空字符串是最优解：满足 schema、零 CPU 开销、与伪造方案功能等价。
    /// 真正的 thinking-level cache 需要 Kiro 上游回传合法 signature 才能启用，
    /// 这是上游未实现的特性，不是 proxy 能补的。
    fn create_thinking_signature_event(&self, index: i32) -> SseEvent {
        SseEvent::new(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {
                    "type": "signature_delta",
                    "signature": ""
                }
            }),
        )
    }

    /// 处理工具使用事件
    pub(super) fn process_tool_use(
        &mut self,
        tool_use: &crate::kiro::model::events::ToolUseEvent,
    ) -> Vec<SseEvent> {
        let mut events = Vec::new();

        self.state_manager.set_has_tool_use(true);

        // tool_use 必须发生在 thinking 结束之后。
        // 但当 `</thinking>` 后面没有 `\n\n`（例如紧跟 tool_use 或流结束）时，
        // thinking 结束标签会滞留在 thinking_buffer，导致后续 flush 时把 `</thinking>` 当作内容输出。
        // 这里在开始 tool_use block 前做一次"边界场景"的结束标签识别与过滤。
        if self.thinking_enabled
            && self.in_thinking_block
            && let Some(end_pos) = find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer)
        {
            let thinking_content = self.thinking_buffer[..end_pos].to_string();
            if let Some(thinking_index) = self.thinking_block_index
                && !thinking_content.is_empty()
            {
                events.push(self.create_thinking_delta_event(thinking_index, &thinking_content));
            }

            // 结束 thinking 块
            self.in_thinking_block = false;
            self.thinking_extracted = true;

            if let Some(thinking_index) = self.thinking_block_index {
                // 先发送空的 thinking_delta
                events.push(self.create_thinking_delta_event(thinking_index, ""));
                // 在 content_block_stop 之前发 signature_delta（Anthropic 规范）
                events.push(self.create_thinking_signature_event(thinking_index));
                // 再发送 content_block_stop
                if let Some(stop_event) =
                    self.state_manager.handle_content_block_stop(thinking_index)
                {
                    events.push(stop_event);
                }
            }

            // 把结束标签后的内容当作普通文本（通常为空或空白）
            let after_pos = end_pos + "</thinking>".len();
            let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
            self.thinking_buffer.clear();
            if !remaining.is_empty() {
                events.extend(self.create_text_delta_events(&remaining));
            }
        }

        // thinking 模式下，process_content_with_thinking 可能会为了探测 `<thinking>` 而暂存一小段尾部文本。
        // 如果此时直接开始 tool_use，状态机会自动关闭 text block，导致这段"待输出文本"看起来被 tool_use 吞掉。
        // 约束：只在尚未进入 thinking block、且 thinking 尚未被提取时，将缓冲区当作普通文本 flush。
        if self.thinking_enabled
            && !self.in_thinking_block
            && !self.thinking_extracted
            && !self.thinking_buffer.is_empty()
        {
            let buffered = std::mem::take(&mut self.thinking_buffer);
            events.extend(self.create_text_delta_events(&buffered));
        }

        // 获取或分配块索引
        let block_index = if let Some(&idx) = self.tool_block_indices.get(&tool_use.tool_use_id) {
            idx
        } else {
            let idx = self.state_manager.next_block_index();
            self.tool_block_indices
                .insert(tool_use.tool_use_id.clone(), idx);
            idx
        };

        // 还原工具名称（如果有映射）
        let original_name = self
            .tool_name_map
            .get(&tool_use.name)
            .cloned()
            .unwrap_or_else(|| tool_use.name.clone());

        // 发送 content_block_start
        let start_events = self.state_manager.handle_content_block_start(
            block_index,
            "tool_use",
            json!({
                "type": "content_block_start",
                "index": block_index,
                "content_block": {
                    "type": "tool_use",
                    "id": tool_use.tool_use_id,
                    "name": original_name,
                    "input": {}
                }
            }),
        );
        events.extend(start_events);

        // 发送参数增量 (ToolUseEvent.input 是 String 类型)
        if !tool_use.input.is_empty() {
            self.output_tokens += (tool_use.input.len() as i32 + 3) / 4; // 估算 token

            if let Some(delta_event) = self.state_manager.handle_content_block_delta(
                block_index,
                json!({
                    "type": "content_block_delta",
                    "index": block_index,
                    "delta": {
                        "type": "input_json_delta",
                        "partial_json": tool_use.input
                    }
                }),
            ) {
                events.push(delta_event);
            }
        }

        // 如果是完整的工具调用（stop=true），发送 content_block_stop
        if tool_use.stop
            && let Some(stop_event) = self.state_manager.handle_content_block_stop(block_index)
        {
            events.push(stop_event);
        }

        events
    }

    /// 生成最终事件序列
    pub fn generate_final_events(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();

        // 如果只有 reasoning 内容（没有后续 text/tool_use），在流结束前关闭 reasoning 块
        events.extend(self.close_reasoning_if_open());

        // Flush thinking_buffer 中的剩余内容
        if self.thinking_enabled && !self.thinking_buffer.is_empty() {
            if self.in_thinking_block {
                // 末尾可能残留 `</thinking>`（例如紧跟 tool_use 或流结束），需要在 flush 时过滤掉结束标签。
                if let Some(end_pos) =
                    find_real_thinking_end_tag_at_buffer_end(&self.thinking_buffer)
                {
                    let thinking_content = self.thinking_buffer[..end_pos].to_string();
                    if let Some(thinking_index) = self.thinking_block_index
                        && !thinking_content.is_empty()
                    {
                        events.push(
                            self.create_thinking_delta_event(thinking_index, &thinking_content),
                        );
                    }

                    // 关闭 thinking 块：先发送空的 thinking_delta + signature_delta，再发送 content_block_stop
                    if let Some(thinking_index) = self.thinking_block_index {
                        events.push(self.create_thinking_delta_event(thinking_index, ""));
                        events.push(self.create_thinking_signature_event(thinking_index));
                        if let Some(stop_event) =
                            self.state_manager.handle_content_block_stop(thinking_index)
                        {
                            events.push(stop_event);
                        }
                    }

                    // 把结束标签后的内容当作普通文本（通常为空或空白）
                    let after_pos = end_pos + "</thinking>".len();
                    let remaining = self.thinking_buffer[after_pos..].trim_start().to_string();
                    self.thinking_buffer.clear();
                    self.in_thinking_block = false;
                    self.thinking_extracted = true;
                    if !remaining.is_empty() {
                        events.extend(self.create_text_delta_events(&remaining));
                    }
                } else {
                    // 如果还在 thinking 块内，发送剩余内容作为 thinking_delta
                    if let Some(thinking_index) = self.thinking_block_index {
                        events.push(
                            self.create_thinking_delta_event(thinking_index, &self.thinking_buffer),
                        );
                    }
                    // 关闭 thinking 块：先发送空的 thinking_delta，再发送 content_block_stop
                    if let Some(thinking_index) = self.thinking_block_index {
                        // 先发送空的 thinking_delta
                        events.push(self.create_thinking_delta_event(thinking_index, ""));
                        // 在 content_block_stop 之前发 signature_delta（Anthropic 规范）
                        events.push(self.create_thinking_signature_event(thinking_index));
                        // 再发送 content_block_stop
                        if let Some(stop_event) =
                            self.state_manager.handle_content_block_stop(thinking_index)
                        {
                            events.push(stop_event);
                        }
                    }
                }
            } else {
                // 否则发送剩余内容作为 text_delta
                let buffer_content = self.thinking_buffer.clone();
                events.extend(self.create_text_delta_events(&buffer_content));
            }
            self.thinking_buffer.clear();
        }

        // 如果整个流中只产生了 thinking 块，没有 text 也没有 tool_use，
        // 则设置 stop_reason 为 max_tokens（表示模型耗尽了 token 预算在思考上），
        // 并补发一套完整的 text 事件（内容为一个空格），确保 content 数组中有 text 块
        if self.thinking_enabled
            && self.thinking_block_index.is_some()
            && !self.state_manager.has_non_thinking_blocks()
        {
            self.state_manager.set_stop_reason("max_tokens");
            events.extend(self.create_text_delta_events(" "));
        }

        // 始终基于本地估算输入与 cache 统计来生成 usage，
        // 避免因服务端压缩导致上游 token 统计偏低，使客户端误判上下文大小。
        // credit usage 则仅透传上游 meteringEvent，不影响本地 input/cache usage 语义。
        let final_input_tokens = self.input_tokens;
        let billed_input_tokens = self
            .cache_usage
            .map(|cache_usage| {
                billed_input_tokens(
                    final_input_tokens,
                    cache_usage.cache_creation_input_tokens,
                    cache_usage.cache_read_input_tokens,
                )
            })
            .unwrap_or(final_input_tokens);

        #[cfg(feature = "sensitive-logs")]
        tracing::info!(
            estimated_input_tokens = self.input_tokens,
            context_input_tokens = ?self.context_input_tokens,
            final_input_tokens,
            output_tokens = self.output_tokens,
            thinking_tokens = self.thinking_tokens,
            "StreamContext usage: final_input_tokens={} (估算值), billed_input_tokens={}, context_input_tokens={} (上游值), output_tokens={}, thinking_tokens={}",
            final_input_tokens,
            billed_input_tokens,
            self.context_input_tokens.map_or("N/A".to_string(), |v| v.to_string()),
            self.output_tokens,
            self.thinking_tokens
        );

        // 生成最终事件：优先使用上游 tokenUsageEvent 精确值，回退到本地估算
        let final_usage = if let Some(ref tu) = self.token_usage {
            let split = tu.billing_split();
            tracing::info!(
                "tokenUsageEvent 覆盖本地估算: input {} → {}, output {} → {}, cache_read {} → {}, cache_write {} → {}",
                billed_input_tokens, split.input_tokens,
                self.output_tokens, split.output_tokens,
                self.cache_usage.map_or(0, |c| c.cache_read_input_tokens), split.cache_read_input_tokens,
                self.cache_usage.map_or(0, |c| c.cache_creation_input_tokens), split.cache_creation_input_tokens,
            );
            FinalUsage {
                input_tokens: split.input_tokens,
                output_tokens: split.output_tokens,
                thinking_tokens: self.thinking_tokens,
                cache_usage: Some(CacheUsageBreakdown {
                    cache_creation_input_tokens: split.cache_creation_input_tokens,
                    cache_read_input_tokens: split.cache_read_input_tokens,
                    cache_creation_5m_input_tokens: split.cache_creation_input_tokens,
                    cache_creation_1h_input_tokens: 0,
                }),
                metering: self.metering.as_ref(),
            }
        } else {
            FinalUsage {
                input_tokens: billed_input_tokens,
                output_tokens: self.output_tokens,
                thinking_tokens: self.thinking_tokens,
                cache_usage: self.cache_usage,
                metering: self.metering.as_ref(),
            }
        };
        events.extend(self.state_manager.generate_final_events(final_usage));
        events
    }
}

