//! 流式响应处理模块
//!
//! 实现 Kiro → Anthropic 流式响应转换和 SSE 状态管理

mod context;
mod event;
mod state;
mod thinking;
pub(crate) mod tool_json;
mod usage;

// Re-export public API
pub use context::StreamContext;
pub use event::SseEvent;
pub(crate) use tool_json::ToolJsonAccumulator;
pub(crate) use usage::CacheUsageBreakdown;
pub(crate) const THINKING_SIGNATURE_PLACEHOLDER: &str = "kiro-rs-thinking-signature";
#[cfg(test)]
mod tests {
    use super::state::SseStateManager;
    use super::thinking::{find_real_thinking_end_tag, find_real_thinking_start_tag};
    use super::usage::estimate_tokens;
    use super::*;
    use crate::kiro::model::events::{Event, MeteringEvent};
    use serde_json::json;
    use std::collections::HashMap;

    fn zero_cache_usage() -> Option<CacheUsageBreakdown> {
        Some(CacheUsageBreakdown {
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        })
    }

    fn cache_usage(
        cache_creation_input_tokens: i32,
        cache_read_input_tokens: i32,
    ) -> Option<CacheUsageBreakdown> {
        Some(CacheUsageBreakdown {
            cache_creation_input_tokens,
            cache_read_input_tokens,
        })
    }

    /// Round 7 regression: "error" must be the highest-priority stop_reason,
    /// covering end_turn / tool_use / max_tokens / model_context_window_exceeded.
    /// 上游流中断 (handlers.rs Some(Err(e)) 分支) 与 Event::Error/Exception 都靠
    /// 这个优先级覆盖默认 end_turn —— Round 5 patch #11-13 的回归保护。
    #[test]
    fn test_stop_reason_priority_error_wins_over_end_turn() {
        let mut mgr = SseStateManager::new();
        // 先设了 end_turn，再设 error 应该覆盖。
        mgr.set_stop_reason("end_turn");
        mgr.set_stop_reason("error");
        // 模拟 final delta 抽出最终 stop_reason。
        let final_reason = mgr.stop_reason.clone();
        assert_eq!(final_reason.as_deref(), Some("error"));
    }

    #[test]
    fn test_stop_reason_priority_error_wins_over_tool_use_and_max_tokens() {
        for first_set in &["tool_use", "max_tokens", "model_context_window_exceeded"] {
            let mut mgr = SseStateManager::new();
            mgr.set_stop_reason(*first_set);
            mgr.set_stop_reason("error");
            assert_eq!(
                mgr.stop_reason.as_deref(),
                Some("error"),
                "error must override {}",
                first_set
            );
        }
    }

    /// Round 7 regression: Event::Error gets converted to an Anthropic SSE error
    /// event AND set stop_reason="error" — Round 5 patch #13.
    #[test]
    fn test_event_error_emits_sse_error_event() {
        let mut ctx =
            StreamContext::new_with_thinking("claude-sonnet-4", 0, None, false, HashMap::new());
        let events = ctx.process_kiro_event(&Event::Error {
            error_code: "ThrottlingException".to_string(),
            error_message: "Rate limited".to_string(),
        });
        assert_eq!(
            events.len(),
            1,
            "Event::Error should emit one SSE error event"
        );
        assert_eq!(events[0].event, "error");
        let data = &events[0].data;
        assert_eq!(data["type"], "error");
        assert_eq!(data["error"]["type"], "ThrottlingException");
        assert_eq!(data["error"]["message"], "Rate limited");
        assert_eq!(ctx.state_manager.stop_reason.as_deref(), Some("error"));
    }

    /// Round 7 regression: Event::Exception (non ContentLengthExceededException)
    /// also flows through the new "error" SSE path.
    #[test]
    fn test_event_exception_emits_sse_error_event() {
        let mut ctx =
            StreamContext::new_with_thinking("claude-sonnet-4", 0, None, false, HashMap::new());
        let events = ctx.process_kiro_event(&Event::Exception {
            exception_type: "InternalServerException".to_string(),
            message: "boom".to_string(),
        });
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "error");
        assert_eq!(events[0].data["error"]["type"], "InternalServerException");
        assert_eq!(ctx.state_manager.stop_reason.as_deref(), Some("error"));
    }

    /// Round 7 regression: ContentLengthExceededException stays on its
    /// "max_tokens" override (NOT mapped to "error"), preserving Round 5's
    /// special-case for the more semantically-precise stop_reason.
    #[test]
    fn test_event_content_length_exceeded_keeps_max_tokens() {
        let mut ctx =
            StreamContext::new_with_thinking("claude-sonnet-4", 0, None, false, HashMap::new());
        let events = ctx.process_kiro_event(&Event::Exception {
            exception_type: "ContentLengthExceededException".to_string(),
            message: "too long".to_string(),
        });
        assert!(
            events.is_empty(),
            "ContentLengthExceededException emits no SSE event"
        );
        assert_eq!(ctx.state_manager.stop_reason.as_deref(), Some("max_tokens"));
    }

    #[test]
    fn test_sse_event_format() {
        let event = SseEvent::new("message_start", json!({"type": "message_start"}));
        let sse_str = event.to_sse_string();

        assert!(sse_str.starts_with("event: message_start\n"));
        assert!(sse_str.contains("data: "));
        assert!(sse_str.ends_with("\n\n"));
    }

    #[test]
    fn test_sse_state_manager_message_start() {
        let mut manager = SseStateManager::new();

        // 第一次应该成功
        let event = manager.handle_message_start(json!({"type": "message_start"}));
        assert!(event.is_some());

        // 第二次应该被跳过
        let event = manager.handle_message_start(json!({"type": "message_start"}));
        assert!(event.is_none());
    }

    #[test]
    fn test_sse_state_manager_block_lifecycle() {
        let mut manager = SseStateManager::new();

        // 创建块
        let events = manager.handle_content_block_start(0, "text", json!({}));
        assert_eq!(events.len(), 1);

        // delta
        let event = manager.handle_content_block_delta(0, json!({}));
        assert!(event.is_some());

        // stop
        let event = manager.handle_content_block_stop(0);
        assert!(event.is_some());

        // 重复 stop 应该被跳过
        let event = manager.handle_content_block_stop(0);
        assert!(event.is_none());
    }

    #[test]
    fn test_stream_context_includes_metering_in_message_delta_usage() {
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            123,
            zero_cache_usage(),
            false,
            HashMap::new(),
        );
        let initial_events = ctx.generate_initial_events();
        let mut all_events = initial_events;
        ctx.process_kiro_event(&Event::Metering(MeteringEvent {
            unit: "credit".to_string(),
            unit_plural: "credits".to_string(),
            usage: 0.75,
        }));

        all_events.extend(ctx.generate_final_events());
        let message_start_usage = all_events
            .iter()
            .find(|e| e.event == "message_start")
            .map(|e| e.data["message"]["usage"].clone())
            .expect("message_start should exist");

        let message_delta_usage = all_events
            .iter()
            .find(|e| e.event == "message_delta")
            .map(|e| e.data["usage"].clone())
            .expect("message_delta should exist");

        assert_eq!(message_start_usage["input_tokens"], json!(123));
        assert_eq!(message_start_usage["cache_creation_input_tokens"], json!(0));
        assert_eq!(message_start_usage["cache_read_input_tokens"], json!(0));
        // message_delta.usage 只含 output_tokens（Anthropic 标准）
        assert!(message_delta_usage.get("input_tokens").is_none());
        assert!(message_delta_usage.get("credit_usage").is_none());
    }

    #[test]
    fn test_stream_context_includes_cache_usage_fields() {
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            321,
            cache_usage(7, 8),
            false,
            HashMap::new(),
        );
        let all_events = ctx.generate_initial_events();

        let message_start_usage = all_events
            .iter()
            .find(|e| e.event == "message_start")
            .map(|e| e.data["message"]["usage"].clone())
            .expect("message_start should exist");

        assert_eq!(message_start_usage["input_tokens"], json!(306));
        assert_eq!(message_start_usage["cache_creation_input_tokens"], json!(7));
        assert_eq!(message_start_usage["cache_read_input_tokens"], json!(8));
    }
    #[test]
    fn test_stream_context_uses_billed_input_tokens_when_cache_read_present() {
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            321,
            cache_usage(7, 8),
            false,
            HashMap::new(),
        );
        let initial_events = ctx.generate_initial_events();
        let message_start_usage = initial_events
            .iter()
            .find(|e| e.event == "message_start")
            .map(|e| e.data["message"]["usage"].clone())
            .expect("message_start should exist");

        let final_events = ctx.generate_final_events();
        let message_delta_usage = final_events
            .iter()
            .find(|e| e.event == "message_delta")
            .map(|e| e.data["usage"].clone())
            .expect("message_delta should exist");

        assert_eq!(message_start_usage["input_tokens"], json!(306));
        // message_delta.usage 只含 output_tokens（Anthropic 标准）
        assert!(message_delta_usage.get("input_tokens").is_none());
        assert!(
            message_delta_usage
                .get("cache_creation_input_tokens")
                .is_none()
        );
    }
    #[test]
    fn test_stream_context_omits_cache_usage_fields_when_disabled() {
        let mut ctx =
            StreamContext::new_with_thinking("test-model", 321, None, false, HashMap::new());
        let initial_events = ctx.generate_initial_events();
        let message_start_usage = initial_events
            .iter()
            .find(|e| e.event == "message_start")
            .map(|e| e.data["message"]["usage"].clone())
            .expect("message_start should exist");

        let final_events = ctx.generate_final_events();
        let message_delta_usage = final_events
            .iter()
            .find(|e| e.event == "message_delta")
            .map(|e| e.data["usage"].clone())
            .expect("message_delta should exist");

        assert_eq!(message_start_usage["input_tokens"], json!(321));
        assert!(
            message_start_usage
                .get("cache_creation_input_tokens")
                .is_none()
        );
        assert!(message_start_usage.get("cache_read_input_tokens").is_none());
        // message_delta.usage 只含 output_tokens
        assert!(message_delta_usage.get("input_tokens").is_none());
        assert!(
            message_delta_usage
                .get("cache_creation_input_tokens")
                .is_none()
        );
        assert!(message_delta_usage.get("cache_creation").is_none());
    }

    #[test]
    fn test_stream_context_extracts_cache_from_metering_event() {
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1000,
            cache_usage(50, 800),
            false,
            HashMap::new(),
        );

        ctx.process_kiro_event(&Event::Metering(MeteringEvent {
            unit: "credit".to_string(),
            unit_plural: "credits".to_string(),
            usage: 0.5,
        }));

        let final_events = ctx.generate_final_events();
        let message_delta_usage = final_events
            .iter()
            .find(|e| e.event == "message_delta")
            .map(|e| e.data["usage"].clone())
            .expect("message_delta should exist");

        // message_delta.usage 只含 output_tokens（Anthropic 标准）
        assert!(message_delta_usage.get("input_tokens").is_none());
        assert!(message_delta_usage.get("cache_read_input_tokens").is_none());
        assert!(message_delta_usage.get("cache_creation").is_none());
        assert!(message_delta_usage.get("credit_usage").is_none());
        assert!(message_delta_usage["output_tokens"].is_number());
    }

    #[test]
    fn test_text_delta_after_tool_use_restarts_text_block() {
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            false,
            HashMap::new(),
        );

        let initial_events = ctx.generate_initial_events();
        assert!(
            initial_events
                .iter()
                .all(|e| !(e.event == "content_block_start"
                    && e.data["content_block"]["type"] == "text"))
        );

        // 首次输出文本时应创建 text block
        let first_text_events = ctx.process_assistant_response("hi");
        let initial_text_index = first_text_events
            .iter()
            .find_map(|e| {
                if e.event == "content_block_start" && e.data["content_block"]["type"] == "text" {
                    e.data["index"].as_i64()
                } else {
                    None
                }
            })
            .expect("text block should start on first text delta")
            as i32;

        // tool_use 开始会自动关闭现有 text block
        let tool_events = ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "test_tool".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: "{}".to_string(),
            stop: false,
        });
        assert!(
            tool_events.iter().any(|e| {
                e.event == "content_block_stop"
                    && e.data["index"].as_i64() == Some(initial_text_index as i64)
            }),
            "tool_use should stop the previous text block"
        );

        // 之后再来文本增量，应自动创建新的 text block 而不是往已 stop 的块里写 delta
        let text_events = ctx.process_assistant_response("hello");
        let new_text_start_index = text_events.iter().find_map(|e| {
            if e.event == "content_block_start" && e.data["content_block"]["type"] == "text" {
                e.data["index"].as_i64()
            } else {
                None
            }
        });
        assert!(
            new_text_start_index.is_some(),
            "should start a new text block"
        );
        assert_ne!(
            new_text_start_index.unwrap(),
            initial_text_index as i64,
            "new text block index should differ from the stopped one"
        );
        assert!(
            text_events.iter().any(|e| {
                e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "text_delta"
                    && e.data["delta"]["text"] == "hello"
            }),
            "should emit text_delta after restarting text block"
        );
    }

    #[test]
    fn test_tool_use_only_does_not_emit_empty_text_block() {
        // tool_use-only 的流式响应不应产生空 text block（text=""），否则客户端写回 history 会触发上游校验拒绝
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            false,
            HashMap::new(),
        );

        let mut all_events = Vec::new();
        all_events.extend(ctx.generate_initial_events());
        all_events.extend(
            ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
                name: "test_tool".to_string(),
                tool_use_id: "tool_1".to_string(),
                input: "{}".to_string(),
                stop: true,
            }),
        );
        all_events.extend(ctx.generate_final_events());

        assert!(
            all_events.iter().any(|e| {
                e.event == "content_block_start" && e.data["content_block"]["type"] == "tool_use"
            }),
            "should emit tool_use content_block_start"
        );
        assert!(
            all_events.iter().all(|e| {
                !(e.event == "content_block_start" && e.data["content_block"]["type"] == "text")
            }),
            "tool_use-only stream should not start a text block"
        );
    }

    #[test]
    fn test_stream_tool_use_restores_claude_code_name_and_input() {
        let mut tool_name_map = HashMap::new();
        tool_name_map.insert("fs_write".to_string(), "Write".to_string());
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            false,
            tool_name_map,
        );

        let events = ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "fs_write".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: r#"{"path":"/tmp/a.txt","text":"hello"}"#.to_string(),
            stop: true,
        });

        let start = events
            .iter()
            .find(|event| event.event == "content_block_start")
            .unwrap();
        assert_eq!(start.data["content_block"]["name"], "Write");

        let delta = events
            .iter()
            .find(|event| event.data["delta"]["type"] == "input_json_delta")
            .unwrap();
        let input: serde_json::Value =
            serde_json::from_str(delta.data["delta"]["partial_json"].as_str().unwrap()).unwrap();
        assert_eq!(
            input,
            serde_json::json!({"file_path": "/tmp/a.txt", "content": "hello"})
        );
    }

    #[test]
    fn test_tool_use_flushes_pending_thinking_buffer_text_before_tool_block() {
        // thinking 模式下，短文本可能被暂存在 thinking_buffer 以等待 `<thinking>` 的跨 chunk 匹配。
        // 当紧接着出现 tool_use 时，应先 flush 这段文本，再开始 tool_use block。
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        // 两段短文本（各 2 个中文字符），总长度仍可能不足以满足 safe_len>0 的输出条件，
        // 因而会留在 thinking_buffer 中等待后续 chunk。
        let ev1 = ctx.process_assistant_response("有修");
        assert!(
            ev1.iter().all(|e| e.event != "content_block_delta"),
            "short prefix should be buffered under thinking mode"
        );
        let ev2 = ctx.process_assistant_response("改：");
        assert!(
            ev2.iter().all(|e| e.event != "content_block_delta"),
            "short prefix should still be buffered under thinking mode"
        );

        let events = ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "Write".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: "{}".to_string(),
            stop: false,
        });

        let text_start_index = events.iter().find_map(|e| {
            if e.event == "content_block_start" && e.data["content_block"]["type"] == "text" {
                e.data["index"].as_i64()
            } else {
                None
            }
        });
        let pos_text_delta = events.iter().position(|e| {
            e.event == "content_block_delta" && e.data["delta"]["type"] == "text_delta"
        });
        let pos_text_stop = text_start_index.and_then(|idx| {
            events.iter().position(|e| {
                e.event == "content_block_stop" && e.data["index"].as_i64() == Some(idx)
            })
        });
        let pos_tool_start = events.iter().position(|e| {
            e.event == "content_block_start" && e.data["content_block"]["type"] == "tool_use"
        });

        assert!(
            text_start_index.is_some(),
            "should start a text block to flush buffered text"
        );
        assert!(
            pos_text_delta.is_some(),
            "should flush buffered text as text_delta"
        );
        assert!(
            pos_text_stop.is_some(),
            "should stop text block before tool_use block starts"
        );
        assert!(pos_tool_start.is_some(), "should start tool_use block");

        let pos_text_delta = pos_text_delta.unwrap();
        let pos_text_stop = pos_text_stop.unwrap();
        let pos_tool_start = pos_tool_start.unwrap();

        assert!(
            pos_text_delta < pos_text_stop && pos_text_stop < pos_tool_start,
            "ordering should be: text_delta -> text_stop -> tool_use_start"
        );

        assert!(
            events.iter().any(|e| {
                e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "text_delta"
                    && e.data["delta"]["text"] == "有修改："
            }),
            "flushed text should equal the buffered prefix"
        );
    }

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("Hello") > 0);
        assert!(estimate_tokens("你好") > 0);
        assert!(estimate_tokens("Hello 你好") > 0);
    }

    #[test]
    fn test_find_real_thinking_start_tag_basic() {
        // 基本情况：正常的开始标签
        assert_eq!(find_real_thinking_start_tag("<thinking>"), Some(0));
        assert_eq!(find_real_thinking_start_tag("prefix<thinking>"), Some(6));
    }

    #[test]
    fn test_find_real_thinking_start_tag_with_backticks() {
        // 被反引号包裹的应该被跳过
        assert_eq!(find_real_thinking_start_tag("`<thinking>`"), None);
        assert_eq!(find_real_thinking_start_tag("use `<thinking>` tag"), None);

        // 先有被包裹的，后有真正的开始标签
        assert_eq!(
            find_real_thinking_start_tag("about `<thinking>` tag<thinking>content"),
            Some(22)
        );
    }

    #[test]
    fn test_find_real_thinking_start_tag_with_quotes() {
        // 被双引号包裹的应该被跳过
        assert_eq!(find_real_thinking_start_tag("\"<thinking>\""), None);
        assert_eq!(find_real_thinking_start_tag("the \"<thinking>\" tag"), None);

        // 被单引号包裹的应该被跳过
        assert_eq!(find_real_thinking_start_tag("'<thinking>'"), None);

        // 混合情况
        assert_eq!(
            find_real_thinking_start_tag("about \"<thinking>\" and '<thinking>' then<thinking>"),
            Some(40)
        );
    }

    #[test]
    fn test_find_real_thinking_end_tag_basic() {
        // 基本情况：正常的结束标签后面有双换行符
        assert_eq!(find_real_thinking_end_tag("</thinking>\n\n"), Some(0));
        assert_eq!(
            find_real_thinking_end_tag("content</thinking>\n\n"),
            Some(7)
        );
        assert_eq!(
            find_real_thinking_end_tag("some text</thinking>\n\nmore text"),
            Some(9)
        );

        // 没有双换行符的情况
        assert_eq!(find_real_thinking_end_tag("</thinking>"), None);
        assert_eq!(find_real_thinking_end_tag("</thinking>\n"), None);
        assert_eq!(find_real_thinking_end_tag("</thinking> more"), None);
    }

    #[test]
    fn test_find_real_thinking_end_tag_with_backticks() {
        // 被反引号包裹的应该被跳过
        assert_eq!(find_real_thinking_end_tag("`</thinking>`\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("mention `</thinking>` in code\n\n"),
            None
        );

        // 只有前面有反引号
        assert_eq!(find_real_thinking_end_tag("`</thinking>\n\n"), None);

        // 只有后面有反引号
        assert_eq!(find_real_thinking_end_tag("</thinking>`\n\n"), None);
    }

    #[test]
    fn test_find_real_thinking_end_tag_with_quotes() {
        // 被双引号包裹的应该被跳过
        assert_eq!(find_real_thinking_end_tag("\"</thinking>\"\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("the string \"</thinking>\" is a tag\n\n"),
            None
        );

        // 被单引号包裹的应该被跳过
        assert_eq!(find_real_thinking_end_tag("'</thinking>'\n\n"), None);
        assert_eq!(
            find_real_thinking_end_tag("use '</thinking>' as marker\n\n"),
            None
        );

        // 混合情况：双引号包裹后有真正的标签
        assert_eq!(
            find_real_thinking_end_tag("about \"</thinking>\" tag</thinking>\n\n"),
            Some(23)
        );

        // 混合情况：单引号包裹后有真正的标签
        assert_eq!(
            find_real_thinking_end_tag("about '</thinking>' tag</thinking>\n\n"),
            Some(23)
        );
    }

    #[test]
    fn test_find_real_thinking_end_tag_mixed() {
        // 先有被包裹的，后有真正的结束标签
        assert_eq!(
            find_real_thinking_end_tag("discussing `</thinking>` tag</thinking>\n\n"),
            Some(28)
        );

        // 多个被包裹的，最后一个是真正的
        assert_eq!(
            find_real_thinking_end_tag("`</thinking>` and `</thinking>` done</thinking>\n\n"),
            Some(36)
        );

        // 多种引用字符混合
        assert_eq!(
            find_real_thinking_end_tag(
                "`</thinking>` and \"</thinking>\" and '</thinking>' done</thinking>\n\n"
            ),
            Some(54)
        );
    }

    #[test]
    fn test_tool_use_immediately_after_thinking_filters_end_tag_and_closes_thinking_block() {
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();

        // thinking 内容以 `</thinking>` 结尾，但后面没有 `\n\n`（模拟紧跟 tool_use 的场景）
        all_events.extend(ctx.process_assistant_response("<thinking>abc</thinking>"));

        let tool_events = ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
            name: "Write".to_string(),
            tool_use_id: "tool_1".to_string(),
            input: "{}".to_string(),
            stop: false,
        });
        all_events.extend(tool_events);

        all_events.extend(ctx.generate_final_events());

        // 不应把 `</thinking>` 当作 thinking 内容输出
        assert!(
            all_events.iter().all(|e| {
                !(e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "thinking_delta"
                    && e.data["delta"]["thinking"] == "</thinking>")
            }),
            "`</thinking>` should be filtered from output"
        );

        // thinking block 必须在 tool_use block 之前关闭
        let thinking_index = ctx
            .thinking_block_index
            .expect("thinking block index should exist");
        let pos_thinking_stop = all_events.iter().position(|e| {
            e.event == "content_block_stop"
                && e.data["index"].as_i64() == Some(thinking_index as i64)
        });
        let pos_tool_start = all_events.iter().position(|e| {
            e.event == "content_block_start" && e.data["content_block"]["type"] == "tool_use"
        });
        assert!(
            pos_thinking_stop.is_some(),
            "thinking block should be stopped"
        );
        assert!(pos_tool_start.is_some(), "tool_use block should be started");
        assert!(
            pos_thinking_stop.unwrap() < pos_tool_start.unwrap(),
            "thinking block should stop before tool_use block starts"
        );
    }

    #[test]
    fn test_final_flush_filters_standalone_thinking_end_tag() {
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>abc</thinking>"));
        all_events.extend(ctx.generate_final_events());

        assert!(
            all_events.iter().all(|e| {
                !(e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "thinking_delta"
                    && e.data["delta"]["thinking"] == "</thinking>")
            }),
            "`</thinking>` should be filtered during final flush"
        );
    }

    #[test]
    fn test_thinking_strips_leading_newline_same_chunk() {
        // <thinking>\n 在同一个 chunk 中，\n 应被剥离
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let events = ctx.process_assistant_response("<thinking>\nHello world");

        // 找到所有 thinking_delta 事件
        let thinking_deltas: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .collect();

        // 拼接所有 thinking 内容
        let full_thinking: String = thinking_deltas
            .iter()
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .collect();

        assert!(
            !full_thinking.starts_with('\n'),
            "thinking content should not start with \\n, got: {:?}",
            full_thinking
        );
    }

    #[test]
    fn test_thinking_strips_leading_newline_cross_chunk() {
        // <thinking> 在第一个 chunk 末尾，\n 在第二个 chunk 开头
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let events1 = ctx.process_assistant_response("<thinking>");
        let events2 = ctx.process_assistant_response("\nHello world");

        let mut all_events = Vec::new();
        all_events.extend(events1);
        all_events.extend(events2);

        let thinking_deltas: Vec<_> = all_events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .collect();

        let full_thinking: String = thinking_deltas
            .iter()
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .collect();

        assert!(
            !full_thinking.starts_with('\n'),
            "thinking content should not start with \\n across chunks, got: {:?}",
            full_thinking
        );
    }

    #[test]
    fn test_thinking_no_strip_when_no_leading_newline() {
        // <thinking> 后直接跟内容（无 \n），内容应完整保留
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let events = ctx.process_assistant_response("<thinking>abc</thinking>\n\ntext");

        let thinking_deltas: Vec<_> = events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .collect();

        let full_thinking: String = thinking_deltas
            .iter()
            .filter(|e| {
                !e.data["delta"]["thinking"]
                    .as_str()
                    .unwrap_or("")
                    .is_empty()
            })
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .collect();

        assert_eq!(full_thinking, "abc", "thinking content should be 'abc'");
    }

    #[test]
    fn test_text_after_thinking_strips_leading_newlines() {
        // `</thinking>\n\n` 后的文本不应以 \n\n 开头
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let events = ctx.process_assistant_response("<thinking>\nabc</thinking>\n\n你好");

        let text_deltas: Vec<_> = events
            .iter()
            .filter(|e| e.event == "content_block_delta" && e.data["delta"]["type"] == "text_delta")
            .collect();

        let full_text: String = text_deltas
            .iter()
            .map(|e| e.data["delta"]["text"].as_str().unwrap_or(""))
            .collect();

        assert!(
            !full_text.starts_with('\n'),
            "text after thinking should not start with \\n, got: {:?}",
            full_text
        );
        assert_eq!(full_text, "你好");
    }

    /// 辅助函数：从事件列表中提取所有 thinking_delta 的拼接内容
    fn collect_thinking_content(events: &[SseEvent]) -> String {
        events
            .iter()
            .filter(|e| {
                e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta"
            })
            .map(|e| e.data["delta"]["thinking"].as_str().unwrap_or(""))
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// 辅助函数：从事件列表中提取所有 text_delta 的拼接内容
    fn collect_text_content(events: &[SseEvent]) -> String {
        events
            .iter()
            .filter(|e| e.event == "content_block_delta" && e.data["delta"]["type"] == "text_delta")
            .map(|e| e.data["delta"]["text"].as_str().unwrap_or(""))
            .collect()
    }

    #[test]
    fn test_end_tag_newlines_split_across_events() {
        // `</thinking>\n` 在 chunk 1，`\n` 在 chunk 2，`text` 在 chunk 3
        // 确保 `</thinking>` 不会被部分当作 thinking 内容发出
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        all.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>\n"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("你好"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(
            thinking, "abc",
            "thinking should be 'abc', got: {:?}",
            thinking
        );

        let text = collect_text_content(&all);
        assert_eq!(text, "你好", "text should be '你好', got: {:?}", text);
    }

    #[test]
    fn test_end_tag_alone_in_chunk_then_newlines_in_next() {
        // `</thinking>` 单独在一个 chunk，`\n\ntext` 在下一个 chunk
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        all.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>"));
        all.extend(ctx.process_assistant_response("\n\n你好"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(
            thinking, "abc",
            "thinking should be 'abc', got: {:?}",
            thinking
        );

        let text = collect_text_content(&all);
        assert_eq!(text, "你好", "text should be '你好', got: {:?}", text);
    }

    #[test]
    fn test_start_tag_newline_split_across_events() {
        // `\n\n` 在 chunk 1，`<thinking>` 在 chunk 2，`\n` 在 chunk 3
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        all.extend(ctx.process_assistant_response("\n\n"));
        all.extend(ctx.process_assistant_response("<thinking>"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("abc</thinking>\n\ntext"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(
            thinking, "abc",
            "thinking should be 'abc', got: {:?}",
            thinking
        );

        let text = collect_text_content(&all);
        assert_eq!(text, "text", "text should be 'text', got: {:?}", text);
    }

    #[test]
    fn test_full_flow_maximally_split() {
        // 极端拆分：每个关键边界都在不同 chunk
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all = Vec::new();
        // \n\n<thinking>\n 拆成多段
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("<thin"));
        all.extend(ctx.process_assistant_response("king>"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("hello"));
        // </thinking>\n\n 拆成多段
        all.extend(ctx.process_assistant_response("</thi"));
        all.extend(ctx.process_assistant_response("nking>"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("\n"));
        all.extend(ctx.process_assistant_response("world"));
        all.extend(ctx.generate_final_events());

        let thinking = collect_thinking_content(&all);
        assert_eq!(
            thinking, "hello",
            "thinking should be 'hello', got: {:?}",
            thinking
        );

        let text = collect_text_content(&all);
        assert_eq!(text, "world", "text should be 'world', got: {:?}", text);
    }

    #[test]
    fn test_thinking_only_sets_max_tokens_stop_reason() {
        // 整个流只有 thinking 块，没有 text 也没有 tool_use，stop_reason 应为 max_tokens
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>"));
        all_events.extend(ctx.generate_final_events());

        let message_delta = all_events
            .iter()
            .find(|e| e.event == "message_delta")
            .expect("should have message_delta event");

        assert_eq!(
            message_delta.data["delta"]["stop_reason"], "max_tokens",
            "stop_reason should be max_tokens when only thinking is produced"
        );

        // 应补发一套完整的 text 事件（content_block_start + delta 空格 + content_block_stop）
        assert!(
            all_events.iter().any(|e| {
                e.event == "content_block_start" && e.data["content_block"]["type"] == "text"
            }),
            "should emit text content_block_start"
        );
        assert!(
            all_events.iter().any(|e| {
                e.event == "content_block_delta"
                    && e.data["delta"]["type"] == "text_delta"
                    && e.data["delta"]["text"] == " "
            }),
            "should emit text_delta with a single space"
        );
        // text block 应被 generate_final_events 自动关闭
        let text_block_index = all_events
            .iter()
            .find_map(|e| {
                if e.event == "content_block_start" && e.data["content_block"]["type"] == "text" {
                    e.data["index"].as_i64()
                } else {
                    None
                }
            })
            .expect("text block should exist");
        assert!(
            all_events.iter().any(|e| {
                e.event == "content_block_stop"
                    && e.data["index"].as_i64() == Some(text_block_index)
            }),
            "text block should be stopped"
        );
    }

    #[test]
    fn test_thinking_with_text_keeps_end_turn_stop_reason() {
        // thinking + text 的情况，stop_reason 应为 end_turn
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>\n\nHello"));
        all_events.extend(ctx.generate_final_events());

        let message_delta = all_events
            .iter()
            .find(|e| e.event == "message_delta")
            .expect("should have message_delta event");

        assert_eq!(
            message_delta.data["delta"]["stop_reason"], "end_turn",
            "stop_reason should be end_turn when text is also produced"
        );
    }

    #[test]
    fn test_thinking_with_tool_use_keeps_tool_use_stop_reason() {
        // thinking + tool_use 的情况，stop_reason 应为 tool_use
        let mut ctx = StreamContext::new_with_thinking(
            "test-model",
            1,
            zero_cache_usage(),
            true,
            HashMap::new(),
        );
        let _initial_events = ctx.generate_initial_events();

        let mut all_events = Vec::new();
        all_events.extend(ctx.process_assistant_response("<thinking>\nabc</thinking>"));
        all_events.extend(
            ctx.process_tool_use(&crate::kiro::model::events::ToolUseEvent {
                name: "test_tool".to_string(),
                tool_use_id: "tool_1".to_string(),
                input: "{}".to_string(),
                stop: true,
            }),
        );
        all_events.extend(ctx.generate_final_events());

        let message_delta = all_events
            .iter()
            .find(|e| e.event == "message_delta")
            .expect("should have message_delta event");

        assert_eq!(
            message_delta.data["delta"]["stop_reason"], "tool_use",
            "stop_reason should be tool_use when tool_use is present"
        );
    }
}
