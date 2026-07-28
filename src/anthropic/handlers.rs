//! Anthropic API Handler 函数

use std::convert::Infallible;

use crate::admin::trace_db::{TraceAttempt, TraceRecord};
use crate::admin::usage_stats::UsageRecord;
use crate::kiro::model::events::{Event, MeteringEvent};
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::token;

use super::middleware::AuthIdentity;
use anyhow::Error;
use axum::{
    Json as JsonExtractor,
    body::Body,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use serde_json::json;
use std::time::Duration;
use tokio::time::{Instant, interval_at};
use uuid::Uuid;

/// 自适应压缩：最大迭代次数（避免极端输入导致过长 CPU 消耗）
const ADAPTIVE_COMPRESSION_MAX_ITERS: usize = 32;
/// tool_result 二次压缩的最低阈值（字符数）
const ADAPTIVE_MIN_TOOL_RESULT_MAX_CHARS: usize = 512;
/// tool_use input 二次压缩的最低阈值（字符数）
const ADAPTIVE_MIN_TOOL_USE_INPUT_MAX_CHARS: usize = 256;
/// 历史截断默认保留消息数（与 compressor.rs 的 preserve_count 保持一致）
const ADAPTIVE_HISTORY_PRESERVE_MESSAGES: usize = 2;
/// 消息内容二次压缩的最低阈值（字符数）
const ADAPTIVE_MIN_MESSAGE_CONTENT_MAX_CHARS: usize = 8192;

use crate::metrics::{MetricEvent, MetricEventType};

use super::converter::{ConversionError, convert_request_with_mode};
use super::error_map::{self, ErrorRequestContext};
use super::middleware::AppState;

/// 记录请求用量（usage + trace + client key 回写）
pub(crate) struct TelemetryData<'a> {
    pub model: &'a str,
    pub is_stream: bool,
    pub credential_id: u64,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cache_creation_tokens: i32,
    pub cache_read_tokens: i32,
    pub credits: f64,
    pub duration_ms: u64,
    pub status: &'a str,
    pub attempts: Vec<TraceAttempt>,
    pub first_token_ms: Option<u64>,
    pub error_message: Option<String>,
}

pub(crate) fn record_request_telemetry(
    state: &AppState,
    auth: &AuthIdentity,
    data: TelemetryData<'_>,
) {
    let record = UsageRecord {
        ts: chrono::Utc::now().to_rfc3339(),
        key_id: auth.key_id,
        credential_id: data.credential_id,
        model: data.model.to_string(),
        input_tokens: data.input_tokens.max(0) as u64,
        output_tokens: data.output_tokens.max(0) as u64,
        cache_creation_tokens: data.cache_creation_tokens.max(0) as u64,
        cache_read_tokens: data.cache_read_tokens.max(0) as u64,
        credits: data.credits,
        duration_ms: data.duration_ms,
        status: data.status.to_string(),
    };
    if let Some(recorder) = &state.usage_recorder {
        recorder.record(&record);
    }
    if let Some(aggregator) = &state.usage_aggregator {
        aggregator.ingest(&record);
    }
    if let Some(mgr) = &state.client_keys {
        mgr.record_usage(
            auth.key_id,
            record.input_tokens,
            record.output_tokens,
            record.cache_creation_tokens,
            record.cache_read_tokens,
            data.credits,
        );
    }
    if let Some(store) = &state.trace_store
        && store.is_enabled()
    {
        let trace = TraceRecord {
            trace_id: Uuid::new_v4().to_string(),
            ts: record.ts.clone(),
            key_id: auth.key_id,
            key_source: auth.key_source,
            model: data.model.to_string(),
            is_stream: data.is_stream,
            final_status: data.status.to_string(),
            final_credential_id: data.credential_id,
            error_type: if data.status != "success" {
                Some(data.status.to_string())
            } else {
                None
            },
            error_message: data.error_message,
            total_attempts: data.attempts.len().max(1) as u32,
            duration_ms: data.duration_ms,
            interrupted_after_bytes: None,
            input_tokens: record.input_tokens,
            output_tokens: record.output_tokens,
            cache_creation_tokens: record.cache_creation_tokens,
            cache_read_tokens: record.cache_read_tokens,
            credits: data.credits,
            first_token_ms: data.first_token_ms,
            attempts: data.attempts,
        };
        store.insert(&trace);
    }
}

pub(crate) fn last_attempt_credential_id(attempts: &[TraceAttempt]) -> u64 {
    attempts
        .last()
        .map(|attempt| attempt.credential_id)
        .unwrap_or(0)
}

/// 流式路径的 token 用量快照，由 unfold closure 在流结束时写入，
/// 供 handler 层读取后传给 record_request_telemetry。
#[allow(dead_code)]
#[derive(Default)]
struct StreamUsageSnapshot {
    input_tokens: i32,
    output_tokens: i32,
    thinking_tokens: i32,
    cache_creation: i32,
    cache_read: i32,
    credits: f64,
}

use super::stream::{CacheUsageBreakdown, SseEvent, StreamContext};
use super::types::{
    CountTokensRequest, CountTokensResponse, ErrorResponse, MessagesRequest, Model, ModelsResponse,
    OutputConfig, Thinking,
};
use super::websearch;

/// 生成 Anthropic 标准 response headers（完整 rate limit 头集合）
pub(crate) fn anthropic_response_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    let request_id = format!("req_{}", Uuid::new_v4().to_string().replace('-', ""));
    headers.insert("x-request-id", request_id.parse().unwrap());
    headers.insert("request-id", request_id.parse().unwrap());

    let reset_time: String = chrono::Utc::now()
        .checked_add_signed(chrono::Duration::seconds(60))
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    // requests rate limit
    headers.insert(
        "anthropic-ratelimit-requests-limit",
        "4000".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-requests-remaining",
        "3999".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-requests-reset",
        reset_time.parse().unwrap(),
    );

    // combined tokens rate limit
    headers.insert(
        "anthropic-ratelimit-tokens-limit",
        "400000".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-tokens-remaining",
        "399000".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-tokens-reset",
        reset_time.parse().unwrap(),
    );

    // input tokens rate limit
    headers.insert(
        "anthropic-ratelimit-input-tokens-limit",
        "2000000".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-input-tokens-remaining",
        "1999000".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-input-tokens-reset",
        reset_time.parse().unwrap(),
    );

    // output tokens rate limit
    headers.insert(
        "anthropic-ratelimit-output-tokens-limit",
        "400000".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-output-tokens-remaining",
        "399000".parse().unwrap(),
    );
    headers.insert(
        "anthropic-ratelimit-output-tokens-reset",
        reset_time.parse().unwrap(),
    );

    headers
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CacheUsageContext {
    cache_creation_input_tokens: i32,
    cache_read_input_tokens: i32,
    cache_creation_5m_input_tokens: i32,
    cache_creation_1h_input_tokens: i32,
}

struct StreamRequestContext<'a> {
    cache_tracker: Option<&'a std::sync::Arc<crate::anthropic::cache_tracker::CacheTracker>>,
    cache_profile: Option<&'a crate::anthropic::cache_tracker::CacheProfile>,
    request_body: &'a str,
    model: &'a str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    user_id: Option<&'a str>,
    metrics: Option<&'a std::sync::Arc<crate::metrics::MetricsCollector>>,
    request_start: std::time::Instant,
    adaptive_outcome: Option<&'a AdaptiveCompressionOutcome>,
    defer_message_start: bool,
}

struct NonStreamRequestContext<'a> {
    request_body: &'a str,
    model: &'a str,
    input_tokens: i32,
    tool_name_map: std::collections::HashMap<String, String>,
    user_id: Option<&'a str>,
    cache_tracker: Option<&'a std::sync::Arc<crate::anthropic::cache_tracker::CacheTracker>>,
    cache_profile: Option<&'a crate::anthropic::cache_tracker::CacheProfile>,
    metrics: Option<&'a std::sync::Arc<crate::metrics::MetricsCollector>>,
    request_start: std::time::Instant,
    adaptive_outcome: Option<&'a AdaptiveCompressionOutcome>,
}

/// 判断 Kiro request_body 的最新一轮 user 消息是否携带非空 `toolResults`。
///
/// 用于 "空 tool_result 回合" 判别的前置条件——只有在工具结果继续场景下，
/// 上游偶发回一个只有 thinking / 无可见文本的空洞回合时才需要重试。
fn request_is_tool_result_continuation(request_body: &str) -> bool {
    let v: serde_json::Value = match serde_json::from_str(request_body) {
        Ok(v) => v,
        Err(_) => return false,
    };
    v.pointer(
        "/conversationState/currentMessage/userInputMessage/userInputMessageContext/toolResults",
    )
    .and_then(|arr| arr.as_array())
    .map(|arr| !arr.is_empty())
    .unwrap_or(false)
}

/// 判断解析出的助手回合是否 "空洞继续"：无可见文本、无工具调用、无终止原因。
///
/// 纯 thinking / reasoning 内容不足以构成有效继续——Codex 等下游需要真实
/// assistant 文本或 client tool_use 才能保持任务生命周期正确。
fn is_empty_assistant_turn(cleaned_text: &str, tool_uses_len: usize, stop_reason: &str) -> bool {
    cleaned_text.trim().is_empty() && tool_uses_len == 0 && stop_reason == "end_turn"
}

fn build_cache_profile(
    cache_tracker: &crate::anthropic::cache_tracker::CacheTracker,
    payload: &MessagesRequest,
    total_input_tokens: i32,
) -> crate::anthropic::cache_tracker::CacheProfile {
    cache_tracker.build_profile(payload, total_input_tokens)
}

fn compute_cache_usage(
    cache_tracker: &crate::anthropic::cache_tracker::CacheTracker,
    credential_id: u64,
    profile: &crate::anthropic::cache_tracker::CacheProfile,
) -> CacheUsageContext {
    let result = cache_tracker.compute(credential_id, profile);
    CacheUsageContext {
        cache_creation_input_tokens: result.cache_creation_input_tokens,
        cache_read_input_tokens: result.cache_read_input_tokens,
        cache_creation_5m_input_tokens: result.cache_creation_5m_input_tokens,
        cache_creation_1h_input_tokens: result.cache_creation_1h_input_tokens,
    }
}

fn provisional_cache_usage(
    cache_tracker: &crate::anthropic::cache_tracker::CacheTracker,
    profile: &crate::anthropic::cache_tracker::CacheProfile,
) -> CacheUsageContext {
    compute_cache_usage(cache_tracker, 0, profile)
}

fn resolved_cache_usage(
    cache_tracker: &crate::anthropic::cache_tracker::CacheTracker,
    credential_id: u64,
    profile: &crate::anthropic::cache_tracker::CacheProfile,
) -> CacheUsageContext {
    compute_cache_usage(cache_tracker, credential_id, profile)
}

fn inject_cache_usage_fields(usage: &mut serde_json::Value, cache_context: CacheUsageContext) {
    usage["cache_creation_input_tokens"] = json!(cache_context.cache_creation_input_tokens);
    usage["cache_read_input_tokens"] = json!(cache_context.cache_read_input_tokens);
    // cache_creation 嵌套对象为非 Anthropic 标准字段，不注入
}

fn billed_input_tokens(
    input_tokens: i32,
    cache_creation_input_tokens: i32,
    cache_read_input_tokens: i32,
) -> i32 {
    input_tokens
        .saturating_sub(cache_creation_input_tokens)
        .saturating_sub(cache_read_input_tokens)
        .max(0)
}

fn stream_telemetry_input_tokens(ctx: &StreamContext) -> i32 {
    if let Some(ref token_usage) = ctx.token_usage {
        return token_usage.billing_split().input_tokens;
    }

    ctx.cache_usage
        .map(|cache_usage| {
            billed_input_tokens(
                ctx.input_tokens,
                cache_usage.cache_creation_input_tokens,
                cache_usage.cache_read_input_tokens,
            )
        })
        .unwrap_or(ctx.input_tokens)
}

fn inject_credit_usage_fields(_usage: &mut serde_json::Value, _metering: &MeteringEvent) {
    // credit 字段不注入到 Anthropic 标准 usage（非标准字段会导致 API 兼容性检测失败）
    // 计费数据通过 telemetry/trace 记录，不暴露给客户端
}

#[derive(Debug, Default, Clone, Copy)]
struct AdaptiveCompressionOutcome {
    initial_bytes: usize,
    final_bytes: usize,
    iters: usize,
    additional_history_turns_removed: usize,
    final_tool_result_max_chars: usize,
    final_tool_use_input_max_chars: usize,
    final_message_content_max_chars: usize,
}

/// 计算 KiroRequest 中所有图片 base64 数据的总字节数。
///
/// 该统计用于归因请求体大小（图片 base64 往往占用大量 bytes）。
/// 注意：上游存在请求体大小硬限制（约 5MiB），因此图片也必须控制体积；
/// `max_request_body_bytes` 的校验以实际序列化后的总字节数为准。
fn total_image_bytes(kiro_request: &KiroRequest) -> usize {
    let state = &kiro_request.conversation_state;
    let mut total = 0usize;

    // currentMessage 中的图片
    for img in &state.current_message.user_input_message.images {
        total += img.source.bytes.len();
    }

    // 历史消息中的图片
    for msg in &state.history {
        if let crate::kiro::model::requests::conversation::Message::User(user_msg) = msg {
            for img in &user_msg.user_input_message.images {
                total += img.source.bytes.len();
            }
        }
    }

    total
}

fn estimate_request_body_tokens(request_body: &str) -> usize {
    request_body.len() / 4
}

fn adaptive_shrink_request_body(
    kiro_request: &mut KiroRequest,
    base_config: &crate::model::config::CompressionConfig,
    max_body: usize,
    max_tokens: usize,
    request_body: &mut String,
) -> Result<Option<AdaptiveCompressionOutcome>, serde_json::Error> {
    if !base_config.enabled {
        return Ok(None);
    }

    let under_byte_limit = max_body == 0 || request_body.len() <= max_body;
    let under_token_limit =
        max_tokens == 0 || estimate_request_body_tokens(request_body) <= max_tokens;
    if under_byte_limit && under_token_limit {
        return Ok(None);
    }

    let mut outcome = AdaptiveCompressionOutcome {
        initial_bytes: request_body.len(),
        final_bytes: request_body.len(),
        iters: 0,
        additional_history_turns_removed: 0,
        final_tool_result_max_chars: base_config.tool_result_max_chars,
        final_tool_use_input_max_chars: base_config.tool_use_input_max_chars,
        final_message_content_max_chars: 0,
    };

    // 二次压缩策略：
    // 1) 逐步降低 tool_result_max_chars（仅当存在 tool_result/tools）
    // 2) 逐步降低 tool_use_input_max_chars（仅当存在 tool_use）
    // 3) 截断超长用户消息内容（当单条消息已超过阈值时优先）
    // 4) 仅清除一次历史图片（保留 current_message 图片）
    // 5) 按 request_body_bytes 成对移除最老的 user+assistant 两条消息（保留前 2 条）
    //
    // 每轮都会重新跑一次压缩管道（包含 tool 配对修复），再重新序列化计算字节数。
    let mut adaptive_config = base_config.clone();
    let mut history_images_removed = false;

    // 是否存在任何 tool_result / tools（否则降低阈值只会浪费迭代次数）
    let has_any_tool_results_or_tools = {
        let state = &kiro_request.conversation_state;
        if !state
            .current_message
            .user_input_message
            .user_input_message_context
            .tool_results
            .is_empty()
            || !state
                .current_message
                .user_input_message
                .user_input_message_context
                .tools
                .is_empty()
        {
            true
        } else {
            state.history.iter().any(|msg| match msg {
                crate::kiro::model::requests::conversation::Message::User(u) => {
                    !u.user_input_message
                        .user_input_message_context
                        .tool_results
                        .is_empty()
                        || !u
                            .user_input_message
                            .user_input_message_context
                            .tools
                            .is_empty()
                }
                _ => false,
            })
        }
    };

    // 是否存在任何 tool_use（否则降低阈值只会浪费迭代次数）
    let has_any_tool_uses = kiro_request
        .conversation_state
        .history
        .iter()
        .any(|msg| match msg {
            crate::kiro::model::requests::conversation::Message::Assistant(a) => a
                .assistant_response_message
                .tool_uses
                .as_ref()
                .is_some_and(|t| !t.is_empty()),
            _ => false,
        });

    // 是否存在历史图片（否则无需尝试图片降级）
    let has_history_images = kiro_request
        .conversation_state
        .history
        .iter()
        .any(|msg| match msg {
            crate::kiro::model::requests::conversation::Message::User(u) => {
                !u.user_input_message.images.is_empty()
            }
            _ => false,
        });

    // 扫描所有用户消息，找到最大 content 字符数作为初始 message_content_max_chars
    let max_content_chars = {
        let mut max_chars = kiro_request
            .conversation_state
            .current_message
            .user_input_message
            .content
            .chars()
            .count();
        for msg in &kiro_request.conversation_state.history {
            if let crate::kiro::model::requests::conversation::Message::User(u) = msg {
                max_chars = max_chars.max(u.user_input_message.content.chars().count());
            }
        }
        max_chars
    };
    // 初始值设为最大消息字符数的 3/4
    let mut message_content_max_chars =
        (max_content_chars * 3 / 4).max(ADAPTIVE_MIN_MESSAGE_CONTENT_MAX_CHARS);

    for _ in 0..ADAPTIVE_COMPRESSION_MAX_ITERS {
        let under_byte_limit = max_body == 0 || request_body.len() <= max_body;
        let under_token_limit =
            max_tokens == 0 || estimate_request_body_tokens(request_body) <= max_tokens;
        if under_byte_limit && under_token_limit {
            break;
        }

        let mut changed = false;

        if has_any_tool_results_or_tools
            && adaptive_config.tool_result_max_chars > ADAPTIVE_MIN_TOOL_RESULT_MAX_CHARS
        {
            let next = (adaptive_config.tool_result_max_chars * 3 / 4)
                .max(ADAPTIVE_MIN_TOOL_RESULT_MAX_CHARS);
            if next < adaptive_config.tool_result_max_chars {
                adaptive_config.tool_result_max_chars = next;
                changed = true;
            }
        } else if has_any_tool_uses
            && adaptive_config.tool_use_input_max_chars > ADAPTIVE_MIN_TOOL_USE_INPUT_MAX_CHARS
        {
            let next = (adaptive_config.tool_use_input_max_chars * 3 / 4)
                .max(ADAPTIVE_MIN_TOOL_USE_INPUT_MAX_CHARS);
            if next < adaptive_config.tool_use_input_max_chars {
                adaptive_config.tool_use_input_max_chars = next;
                changed = true;
            }
        } else {
            // 如果任意单条 user content 已经超过 max_body，则移除历史并不能让请求落到阈值内，
            // 必须优先截断超长消息内容。
            let max_single_user_content_bytes = {
                let state = &kiro_request.conversation_state;
                let mut max_bytes = state.current_message.user_input_message.content.len();
                for msg in &state.history {
                    if let crate::kiro::model::requests::conversation::Message::User(u) = msg {
                        max_bytes = max_bytes.max(u.user_input_message.content.len());
                    }
                }
                max_bytes
            };

            let history = &mut kiro_request.conversation_state.history;
            if (max_single_user_content_bytes > max_body
                || history.len() <= ADAPTIVE_HISTORY_PRESERVE_MESSAGES + 2)
                && message_content_max_chars >= ADAPTIVE_MIN_MESSAGE_CONTENT_MAX_CHARS
            {
                // 第三层：截断超长消息内容
                let saved = super::compressor::compress_long_messages_pass(
                    &mut kiro_request.conversation_state,
                    message_content_max_chars,
                );
                if saved > 0 {
                    changed = true;
                }
                // 记录本轮实际生效的阈值（递减前）
                outcome.final_message_content_max_chars = message_content_max_chars;
                // 每轮递减 3/4
                message_content_max_chars =
                    (message_content_max_chars * 3 / 4).max(ADAPTIVE_MIN_MESSAGE_CONTENT_MAX_CHARS);
            } else if !history_images_removed && has_history_images {
                // 第四层：仅清除历史图片，保留 current_message 图片
                let removed = kiro_request.conversation_state.remove_history_images();
                if removed > 0 {
                    history_images_removed = true;
                    changed = true;
                }
            } else if history.len() > ADAPTIVE_HISTORY_PRESERVE_MESSAGES + 2 {
                // 第五层：移除最老历史消息（成对移除 user+assistant）
                let preserve = ADAPTIVE_HISTORY_PRESERVE_MESSAGES;
                let min_len = preserve + 2;
                let removable = history.len().saturating_sub(min_len);
                // 单轮最多移除 16 条消息（8 轮），避免一次性丢弃过多上下文
                let mut remove_msgs = removable.min(16);
                remove_msgs -= remove_msgs % 2; // 保持成对移除
                if remove_msgs > 0 {
                    history.drain(preserve..preserve + remove_msgs);
                    outcome.additional_history_turns_removed += remove_msgs / 2;
                    changed = true;
                }
            }
        }

        if !changed {
            break;
        }

        super::compressor::compress(&mut kiro_request.conversation_state, &adaptive_config);
        *request_body = serde_json::to_string(kiro_request)?;
        outcome.iters += 1;
        outcome.final_bytes = request_body.len();
    }

    outcome.final_tool_result_max_chars = adaptive_config.tool_result_max_chars;
    outcome.final_tool_use_input_max_chars = adaptive_config.tool_use_input_max_chars;
    // final_message_content_max_chars 在循环内截断时已记录实际生效值；
    // 若第四层从未执行，保持默认 0 表示未触发

    Ok(Some(outcome))
}

fn map_kiro_provider_error_to_response(
    request_body: &str,
    err: Error,
    adaptive_outcome: Option<&AdaptiveCompressionOutcome>,
) -> Response {
    let ctx = ErrorRequestContext {
        was_compressed: adaptive_outcome.is_some(),
        request_body_bytes: request_body.len(),
        compression_iterations: adaptive_outcome.map(|o| o.iters),
    };
    let category = error_map::classify(&err, &ctx);
    error_map::to_anthropic_response(&category, &err, &ctx)
}

/// 对 user_id 进行掩码处理，保护隐私
fn mask_user_id(user_id: Option<&str>) -> String {
    match user_id {
        Some(id) => {
            let chars: Vec<char> = id.chars().collect();
            let len = chars.len();
            if len > 25 {
                format!(
                    "{}***{}",
                    chars[..13].iter().collect::<String>(),
                    chars[len - 8..].iter().collect::<String>()
                )
            } else if len > 12 {
                format!(
                    "{}***{}",
                    chars[..4].iter().collect::<String>(),
                    chars[len - 4..].iter().collect::<String>()
                )
            } else {
                "***".to_string()
            }
        }
        None => "None".to_string(),
    }
}

/// 剔除 messages 中的空 text content block（`{"type":"text","text":""}` 或纯空白）。
///
/// 说明：
/// - Claude Code/claude-cli 在某些 tool_use-only 场景下可能会把空 text block 写回 history；
///   从 assistant 文本中提取 `<thinking>...</thinking>` XML 标签作为独立 thinking 块。
///
/// Q 上游对非流式请求把推理嵌在 assistantResponseEvent 文本里（不发独立的
/// reasoningContentEvent），需要在非流式响应聚合时拆出来：
///   - 返回 (thinking_text, cleaned_text)
///   - thinking_text 是所有 `<thinking>` 标签内容拼起来
///   - cleaned_text 是去除 thinking 标签和紧随其后空白后的纯回答文本
///
/// 只处理最简单的非嵌套场景；嵌套或畸形标签直接保留原文本不拆分。
fn extract_thinking_xml(text: &str) -> (String, String) {
    const OPEN: &str = "<thinking>";
    const CLOSE: &str = "</thinking>";

    let mut thinking_parts: Vec<String> = Vec::new();
    let mut cleaned = String::with_capacity(text.len());
    let mut cursor = 0usize;

    while let Some(open_rel) = text[cursor..].find(OPEN) {
        let open_abs = cursor + open_rel;
        let content_start = open_abs + OPEN.len();
        let Some(close_rel) = text[content_start..].find(CLOSE) else {
            break;
        };
        let close_abs = content_start + close_rel;

        // 标签前的内容保留到 cleaned
        cleaned.push_str(&text[cursor..open_abs]);
        // 标签内容（trim 两端换行）追加到 thinking_parts
        thinking_parts.push(
            text[content_start..close_abs]
                .trim_matches('\n')
                .to_string(),
        );

        cursor = close_abs + CLOSE.len();
        // 吞掉标签后紧跟的两个换行（模型常用 `</thinking>\n\n` 作分隔符）
        let after = &text[cursor..];
        let strip = after.bytes().take_while(|b| *b == b'\n').count();
        cursor += strip;
    }

    if thinking_parts.is_empty() {
        return (String::new(), text.to_string());
    }

    cleaned.push_str(&text[cursor..]);
    (thinking_parts.join("\n\n"), cleaned.trim().to_string())
}

#[cfg(test)]
mod extract_thinking_tests {
    use super::extract_thinking_xml;

    #[test]
    fn no_tags_returns_original() {
        let (t, c) = extract_thinking_xml("just plain answer");
        assert!(t.is_empty());
        assert_eq!(c, "just plain answer");
    }

    #[test]
    fn single_tag_extracted() {
        let input = "<thinking>\nreasoning here\n</thinking>\n\nFinal answer";
        let (t, c) = extract_thinking_xml(input);
        assert_eq!(t, "reasoning here");
        assert_eq!(c, "Final answer");
    }

    #[test]
    fn unclosed_tag_preserves_original() {
        let input = "<thinking>oops never closed";
        let (t, c) = extract_thinking_xml(input);
        assert!(t.is_empty());
        assert_eq!(c, input);
    }

    #[test]
    fn multiple_tags_joined() {
        let input = "<thinking>first</thinking>\nbetween\n<thinking>second</thinking>\nend";
        let (t, c) = extract_thinking_xml(input);
        assert_eq!(t, "first\n\nsecond");
        assert!(c.contains("between"));
        assert!(c.contains("end"));
    }
}

/// - 上游会拒绝空 text block（400: "text content blocks must be non-empty"）。
/// - 空 text block 不携带任何语义，直接移除是最小且安全的兼容策略。
fn strip_empty_text_content_blocks(messages: &mut [super::types::Message]) -> usize {
    let mut removed = 0usize;

    for msg in messages {
        let serde_json::Value::Array(arr) = &mut msg.content else {
            continue;
        };

        let before = arr.len();
        arr.retain(|item| {
            let Some(obj) = item.as_object() else {
                return true;
            };

            if obj.get("type").and_then(|v| v.as_str()) != Some("text") {
                return true;
            }

            match obj.get("text") {
                Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
                Some(serde_json::Value::Null) | None => false,
                // text 字段类型异常：保守起见不删，交由后续转换/上游校验处理
                _ => true,
            }
        });
        removed += before - arr.len();
    }

    removed
}

/// GET /health
pub async fn health() -> impl IntoResponse {
    Json(json!({"status": "ok"}))
}

/// GET /v1/models
///
/// 返回可用的模型列表。当存在可用凭据时，会额外并发查询各凭据的上游
/// `ListAvailableModels` 目录（走 TTL + singleflight 缓存），合并去重后追加到列表尾部。
pub async fn get_models(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
) -> impl IntoResponse {
    tracing::info!(
        path = %uri.path(),
        "Received request"
    );

    let model_thinking_schema = |model_id: &str| {
        super::converter::thinking_config_for_model(model_id)
            .map(|_| super::converter::output_config_thinking_schema())
    };

    let mut models = vec![
        Model {
            id: "claude-sonnet-5".to_string(),
            object: "model".to_string(),
            created: 1783180800, // Jul 3, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema("claude-sonnet-5"),
        },
        Model {
            id: "claude-sonnet-5-thinking".to_string(),
            object: "model".to_string(),
            created: 1783180800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-sonnet-5-thinking",
            ),
        },
        Model {
            id: "claude-sonnet-5-agentic".to_string(),
            object: "model".to_string(),
            created: 1783180800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 5 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-sonnet-5-agentic",
            ),
        },
        Model {
            id: "claude-sonnet-4-6".to_string(),
            object: "model".to_string(),
            created: 1770314400,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema("claude-sonnet-4-6"),
        },
        Model {
            id: "claude-sonnet-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1770314400,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-sonnet-4-6-thinking",
            ),
        },
        Model {
            id: "claude-sonnet-4-6-agentic".to_string(),
            object: "model".to_string(),
            created: 1770314400,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.6 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-sonnet-4-6-agentic",
            ),
        },
        Model {
            id: "claude-sonnet-4-5-20250929".to_string(),
            object: "model".to_string(),
            created: 1727568000,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-sonnet-4-5-20250929",
            ),
        },
        Model {
            id: "claude-sonnet-4-5-20250929-thinking".to_string(),
            object: "model".to_string(),
            created: 1727568000,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-sonnet-4-5-20250929-thinking",
            ),
        },
        Model {
            id: "claude-sonnet-4-5-20250929-agentic".to_string(),
            object: "model".to_string(),
            created: 1727568000,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Sonnet 4.5 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-sonnet-4-5-20250929-agentic",
            ),
        },
        Model {
            id: "claude-opus-4-5-20251101".to_string(),
            object: "model".to_string(),
            created: 1730419200,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-5-20251101",
            ),
        },
        Model {
            id: "claude-opus-4-5-20251101-thinking".to_string(),
            object: "model".to_string(),
            created: 1730419200,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-5-20251101-thinking",
            ),
        },
        Model {
            id: "claude-opus-4-5-20251101-agentic".to_string(),
            object: "model".to_string(),
            created: 1730419200,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.5 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-5-20251101-agentic",
            ),
        },
        Model {
            id: "claude-opus-4-6".to_string(),
            object: "model".to_string(),
            created: 1770314400,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema("claude-opus-4-6"),
        },
        Model {
            id: "claude-opus-4-6-thinking".to_string(),
            object: "model".to_string(),
            created: 1770314400,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-6-thinking",
            ),
        },
        Model {
            id: "claude-opus-4-6-agentic".to_string(),
            object: "model".to_string(),
            created: 1770314400,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.6 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-6-agentic",
            ),
        },
        Model {
            id: "claude-opus-4-7".to_string(),
            object: "model".to_string(),
            created: 1772992800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema("claude-opus-4-7"),
        },
        Model {
            id: "claude-opus-4-7-thinking".to_string(),
            object: "model".to_string(),
            created: 1772992800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-7-thinking",
            ),
        },
        Model {
            id: "claude-opus-4-7-agentic".to_string(),
            object: "model".to_string(),
            created: 1772992800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.7 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-7-agentic",
            ),
        },
        Model {
            id: "claude-opus-4-8".to_string(),
            object: "model".to_string(),
            created: 1779897600, // May 28, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.8".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema("claude-opus-4-8"),
        },
        Model {
            id: "claude-opus-4-8-thinking".to_string(),
            object: "model".to_string(),
            created: 1779897600, // May 28, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.8 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-8-thinking",
            ),
        },
        Model {
            id: "claude-opus-4-8-agentic".to_string(),
            object: "model".to_string(),
            created: 1779897600, // May 28, 2026
            owned_by: "anthropic".to_string(),
            display_name: "Claude Opus 4.8 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(1_000_000),
            max_completion_tokens: Some(128_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-opus-4-8-agentic",
            ),
        },
        Model {
            id: "claude-haiku-4-5-20251001".to_string(),
            object: "model".to_string(),
            created: 1727740800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-haiku-4-5-20251001",
            ),
        },
        Model {
            id: "claude-haiku-4-5-20251001-thinking".to_string(),
            object: "model".to_string(),
            created: 1727740800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5 (Thinking)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-haiku-4-5-20251001-thinking",
            ),
        },
        Model {
            id: "claude-haiku-4-5-20251001-agentic".to_string(),
            object: "model".to_string(),
            created: 1727740800,
            owned_by: "anthropic".to_string(),
            display_name: "Claude Haiku 4.5 (Agentic)".to_string(),
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length: Some(200_000),
            max_completion_tokens: Some(64_000),
            thinking: Some(true),
            additional_model_request_fields_schema: model_thinking_schema(
                "claude-haiku-4-5-20251001-agentic",
            ),
        },
    ];

    append_runtime_gateway_models(&mut models);
    append_dynamic_upstream_models(&state, &mut models).await;
    append_custom_models(&mut models);

    Json(ModelsResponse {
        object: "list".to_string(),
        data: models,
    })
}

/// 并发查询各未禁用凭据的上游模型目录（走缓存），合并去重后追加到列表尾部。
///
/// 失败凭据静默跳过（缓存层已提供 stale 兜底）；无 provider / 无凭据时 no-op。
async fn append_dynamic_upstream_models(state: &AppState, models: &mut Vec<Model>) {
    let Some(provider) = state.kiro_provider.as_ref() else {
        return;
    };
    let manager = provider.token_manager();
    let ids = manager.list_enabled_credential_ids();
    if ids.is_empty() {
        return;
    }

    // 并发拉各凭据的缓存目录
    let futures = ids
        .into_iter()
        .map(|id| async move { manager.get_available_models_cached(id).await.ok() });
    let responses = futures::future::join_all(futures).await;

    // 汇总所有上游模型，按 model_id 去重（保留首次出现的元数据）
    use std::collections::BTreeMap;
    let mut merged: BTreeMap<String, crate::kiro::model::available_models::UpstreamModel> =
        BTreeMap::new();
    for resp in responses.into_iter().flatten() {
        for m in resp.models {
            merged.entry(m.model_id.clone()).or_insert(m);
        }
    }

    for (id, upstream) in merged {
        if models
            .iter()
            .any(|existing| existing.id.eq_ignore_ascii_case(&id))
        {
            continue;
        }
        let display_name = upstream.model_name.clone().unwrap_or_else(|| id.clone());
        let context_length = upstream
            .token_limits
            .as_ref()
            .and_then(|limits| limits.max_input_tokens)
            .map(|t| t.min(i32::MAX as i64) as i32);
        let max_completion = Some(64_000);
        let thinking = if id.starts_with("claude-") {
            Some(true)
        } else {
            None
        };
        models.push(Model {
            id,
            object: "model".to_string(),
            created: 1783180800,
            owned_by: "kiro".to_string(),
            display_name,
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length,
            max_completion_tokens: max_completion,
            thinking,
            additional_model_request_fields_schema: None,
        });
    }
}

/// 追加 `config.custom_models` 中的自定义模型。
///
/// 保持配置文件中的原始顺序；`id` 大小写不敏感冲突时跳过（内置项优先）。
fn append_custom_models(models: &mut Vec<Model>) {
    let now = 1783180800; // Jul 3, 2026 (与最新内置模型时间戳对齐)
    for cm in crate::model::custom_models::all() {
        if models
            .iter()
            .any(|existing| existing.id.eq_ignore_ascii_case(&cm.id))
        {
            continue;
        }
        let display_name = cm.display_name.clone().unwrap_or_else(|| cm.id.clone());
        let owned_by = cm.owned_by.clone().unwrap_or_else(|| "custom".to_string());
        let context_length = cm.context_window.or(Some(200_000));
        let max_completion = cm.max_tokens.or(Some(64_000));
        models.push(Model {
            id: cm.id.clone(),
            object: "model".to_string(),
            created: now,
            owned_by,
            display_name,
            model_type: "chat".to_string(),
            max_tokens: 32000,
            context_length,
            max_completion_tokens: max_completion,
            thinking: cm.supports_reasoning,
            additional_model_request_fields_schema: None,
        });
    }
}

fn append_runtime_gateway_models(models: &mut Vec<Model>) {
    for model in runtime_gateway_models_for_models_endpoint() {
        if !models.iter().any(|existing| existing.id == model.id) {
            models.push(model);
        }
    }
}

fn runtime_gateway_models_for_models_endpoint() -> Vec<Model> {
    crate::kiro::model::available_models::runtime_fallback_models()
        .models
        .into_iter()
        .map(|upstream| {
            let list_id = if upstream.model_id == "auto" {
                // 与 kiro-gateway 对齐：`auto` 仍可直连，但列表中展示避开 IDE 冲突的别名。
                "auto-kiro".to_string()
            } else {
                upstream.model_id
            };
            let display_name = upstream.model_name.unwrap_or_else(|| list_id.clone());
            let context_length = upstream
                .token_limits
                .and_then(|limits| limits.max_input_tokens)
                .map(|tokens| tokens.min(i32::MAX as i64) as i32);
            let thinking = if list_id.starts_with("claude-") {
                Some(true)
            } else {
                None
            };
            let additional_model_request_fields_schema =
                super::converter::thinking_config_for_model(&list_id)
                    .map(|_| super::converter::output_config_thinking_schema());

            Model {
                id: list_id,
                object: "model".to_string(),
                created: 1779897600,
                owned_by: "kiro".to_string(),
                display_name,
                model_type: "chat".to_string(),
                max_tokens: 32000,
                context_length,
                max_completion_tokens: Some(64_000),
                thinking,
                additional_model_request_fields_schema,
            }
        })
        .collect()
}

/// POST /v1/messages
///
/// 创建消息（对话）
pub async fn post_messages(
    OriginalUri(uri): OriginalUri,
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthIdentity>,
    headers: HeaderMap,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
) -> Response {
    // 读取压缩配置快照（读锁 + clone，避免持锁跨 await）
    let compression_config = state.compression_config.read().clone();
    let prompt_cache = state.prompt_cache_snapshot();

    // Preset 注入：从 x-preset-id header 查找预设，前置注入 system prompt
    if let Some(preset_id) = headers.get("x-preset-id").and_then(|v| v.to_str().ok()) {
        let presets = state.presets.read();
        if let Some(preset) = presets.iter().find(|p| p.id == preset_id && p.enabled) {
            tracing::info!(preset_id = %preset.id, preset_name = %preset.name, "应用 Prompt Preset");
            let preset_msg = super::types::SystemMessage {
                text: preset.system_prompt.clone(),
                block_type: Some("text".to_string()),
                cache_control: None,
            };
            match &mut payload.system {
                Some(system_messages) => {
                    system_messages.insert(0, preset_msg);
                }
                None => {
                    payload.system = Some(vec![preset_msg]);
                }
            }
        } else {
            tracing::debug!(preset_id = %preset_id, "未找到匹配的 Preset 或 Preset 未启用");
        }
    }

    // 预处理：剥离客户端安全限制 + 注入 preset/自定义 system prompt
    super::preprocess::inject_system_prompt(&mut payload, &state.prompt_config);

    // 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
    override_thinking_from_model_name(&mut payload);

    // 提取 user_id 用于凭据亲和性
    let user_id = payload.metadata.as_ref().and_then(|m| m.user_id.clone());

    // 估算压缩前 input tokens（需在 convert_request 之前，因为后者会消费压缩）
    let estimated_input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;

    tracing::info!(
        path = %uri.path(),
        model = %payload.model,
        max_tokens = %payload.max_tokens,
        stream = %payload.stream,
        message_count = %payload.messages.len(),
        user_id = %mask_user_id(user_id.as_deref()),
        estimated_input_tokens,
        "Received request"
    );

    // 短消息内容诊断（单条消息 + < 520 tokens → 可能是测活请求）
    if payload.messages.len() == 1
        && estimated_input_tokens < 520
        && payload.tools.as_ref().map_or(true, |t| t.is_empty())
    {
        let content_preview = match &payload.messages[0].content {
            serde_json::Value::String(s) => s.chars().take(80).collect::<String>(),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b.get("text")?.as_str())
                .take(1)
                .map(|s| s.chars().take(80).collect::<String>())
                .next()
                .unwrap_or_default(),
            _ => String::new(),
        };
        if !content_preview.is_empty() {
            tracing::info!(
                content = %content_preview,
                "短消息内容诊断"
            );
        }
    }

    // 记录 RequestReceived 指标
    if let Some(metrics) = &state.metrics {
        metrics
            .record(MetricEvent::new(MetricEventType::RequestReceived).with_model(&payload.model));
    }

    // 记录请求开始时间（用于计算延迟）
    let request_start = std::time::Instant::now();

    // 测活请求前置过滤：单条问候/探测消息直接模拟回复，不走上游
    let health_check_kind = super::health_check::detect_health_check(&payload);
    if !matches!(
        health_check_kind,
        super::health_check::HealthCheckKind::None
    ) {
        let has_credentials = state
            .kiro_provider
            .as_ref()
            .map(|p| {
                p.token_manager()
                    .available_count_for_group(auth.group.as_deref())
                    > 0
            })
            .unwrap_or(false);

        if !has_credentials {
            tracing::warn!(
                model = %payload.model,
                stream = %payload.stream,
                group = ?auth.group,
                "测活请求拦截 → 分组无可用凭据，返回 401"
            );
            if let Some(metrics) = &state.metrics {
                metrics.record(
                    MetricEvent::new(MetricEventType::RequestCompleted)
                        .with_model(&payload.model)
                        .with_status("error")
                        .with_latency_ms(0),
                );
            }
            record_request_telemetry(
                &state,
                &auth,
                TelemetryData {
                    model: &payload.model,
                    is_stream: payload.stream,
                    credential_id: 0,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    credits: 0.0,
                    duration_ms: 0,
                    status: "no_credentials",
                    attempts: vec![],
                    first_token_ms: None,
                    error_message: None,
                },
            );
            return super::health_check::mock_unauthorized_response();
        }

        tracing::info!(
            model = %payload.model,
            stream = %payload.stream,
            user_id = %mask_user_id(payload.metadata.as_ref().and_then(|m| m.user_id.as_deref())),
            "测活请求已拦截（模拟回复）"
        );
        if let Some(metrics) = &state.metrics {
            metrics.record(
                MetricEvent::new(MetricEventType::RequestCompleted)
                    .with_model(&payload.model)
                    .with_status("success")
                    .with_latency_ms(0),
            );
        }
        record_request_telemetry(
            &state,
            &auth,
            TelemetryData {
                model: &payload.model,
                is_stream: payload.stream,
                credential_id: 0,
                input_tokens: super::health_check::MOCK_INPUT_TOKENS,
                output_tokens: super::health_check::MOCK_OUTPUT_TOKENS,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                credits: 0.0,
                duration_ms: 0,
                status: "success",
                attempts: vec![],
                first_token_ms: Some(0),
                error_message: None,
            },
        );
        return if payload.stream {
            super::health_check::mock_stream_response(&payload.model, &health_check_kind)
        } else {
            super::health_check::mock_non_stream_response(&payload.model, &health_check_kind)
        };
    }

    // 检查 KiroProvider 是否可用
    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            tracing::error!("KiroProvider 未配置");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Kiro API provider not configured",
                )),
            )
                .into_response();
        }
    };

    // 检查是否为纯 WebSearch 请求（仅 web_search 单工具 / tool_choice 强制 / 前缀匹配）
    let websearch_cache_profile = prompt_cache.accounting_enabled.then(|| {
        build_cache_profile(
            prompt_cache.tracker.as_ref(),
            &payload,
            estimated_input_tokens,
        )
    });
    if websearch::should_handle_websearch_request(&payload) {
        tracing::info!("检测到纯 WebSearch 请求，路由到本地 WebSearch 处理");
        return websearch::handle_websearch_request(
            provider,
            &payload,
            if prompt_cache.accounting_enabled {
                Some(&prompt_cache.tracker)
            } else {
                None
            },
            websearch_cache_profile.as_ref(),
            estimated_input_tokens,
            auth.group.as_deref(),
        )
        .await;
    }

    // 混合工具场景：路由到 agentic WebSearch 循环
    if websearch::has_web_search_tool(&payload) {
        tracing::info!("检测到混合工具列表中的 web_search，路由到 agentic WebSearch 循环");
        let is_stream = payload.stream;
        websearch::strip_web_search_tools(&mut payload);
        let compression = state.compression_config.read().clone();
        return super::websearch_loop::run_web_search_loop(
            provider,
            payload,
            is_stream,
            compression,
            auth.group.as_deref(),
            state.tool_compatibility_mode,
        )
        .await;
    }

    // 剔除空 text content block（客户端可能将 tool_use-only 响应中的空 text block 写回 history）
    let stripped = strip_empty_text_content_blocks(&mut payload.messages);
    if stripped > 0 {
        tracing::info!(stripped, "已剔除空 text content block");
    }

    let cache_profile = prompt_cache.accounting_enabled.then(|| {
        build_cache_profile(
            prompt_cache.tracker.as_ref(),
            &payload,
            estimated_input_tokens,
        )
    });
    let provisional_cache_context = cache_profile
        .as_ref()
        .map(|profile| provisional_cache_usage(prompt_cache.tracker.as_ref(), profile))
        .unwrap_or_default();

    tracing::info!(
        provisional_cache_creation_input_tokens =
            provisional_cache_context.cache_creation_input_tokens,
        provisional_cache_read_input_tokens = provisional_cache_context.cache_read_input_tokens,
        cache_accounting_enabled = prompt_cache.accounting_enabled,
        prompt_cache_ttl_seconds = prompt_cache.ttl_seconds,
        "Computed provisional cache usage for /v1/messages"
    );

    // 跨请求缓存查找
    let content_fingerprint = state.cross_request_cache.as_ref().map(|_| {
        super::cross_request_cache::CrossRequestCache::content_fingerprint(&payload.messages)
    });
    let forced_conversation_id = content_fingerprint.as_ref().and_then(|fp| {
        state
            .cross_request_cache
            .as_ref()
            .and_then(|cache| cache.lookup(fp))
    });
    if forced_conversation_id.is_some() {
        tracing::debug!("跨请求缓存命中，将注入 forced_conversation_id");
    }

    // 转换请求
    let conversion_result = match convert_request_with_mode(
        &payload,
        &compression_config,
        forced_conversation_id.as_deref(),
        state.tool_compatibility_mode,
    ) {
        Ok(result) => result,
        Err(e) => {
            let (error_type, message) = match &e {
                ConversionError::UnsupportedModel(model) => {
                    ("invalid_request_error", format!("模型不支持: {}", model))
                }
                ConversionError::EmptyMessages => {
                    ("invalid_request_error", "消息列表为空".to_string())
                }
                ConversionError::EmptyMessageContent => {
                    ("invalid_request_error", "消息内容为空".to_string())
                }
                ConversionError::UnsupportedToolMapping(reason) => (
                    "invalid_request_error",
                    format!("工具映射不支持: {}", reason),
                ),
            };
            tracing::warn!("请求转换失败: {}", e);
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(error_type, message)),
            )
                .into_response();
        }
    };

    // 输出压缩统计（以字节为单位；用于排查上游请求体大小限制，实测约 5MiB 左右会触发 400）
    if let Some(ref stats) = conversion_result.compression_stats {
        tracing::info!(
            estimated_input_tokens,
            bytes_saved_total = stats.total_saved(),
            whitespace_bytes_saved = stats.whitespace_saved,
            thinking_bytes_saved = stats.thinking_saved,
            tool_result_bytes_saved = stats.tool_result_saved,
            tool_use_input_bytes_saved = stats.tool_use_input_saved,
            history_turns_removed = stats.history_turns_removed,
            history_bytes_saved = stats.history_bytes_saved,
            "输入压缩完成"
        );
    }

    // 构建 Kiro 请求
    let tool_name_map = conversion_result.tool_name_map;
    let thinking_config = super::converter::thinking_config_for_model(&payload.model);
    let additional_fields =
        super::converter::build_additional_model_request_fields(&payload, thinking_config.as_ref());
    let mut kiro_request = KiroRequest {
        conversation_state: conversion_result.conversation_state,
        profile_arn: state.profile_arn.clone(),
        additional_model_request_fields: additional_fields,
    };

    let mut request_body = match serde_json::to_string(&kiro_request) {
        Ok(body) => body,
        Err(e) => {
            tracing::error!("序列化请求失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("序列化请求失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    // 请求体大小预检（上游存在硬性请求体大小限制；按实际序列化后的总字节数判断）
    let max_body = compression_config.max_request_body_bytes;
    let max_tokens = compression_config.max_input_tokens;
    let mut adaptive_outcome: Option<AdaptiveCompressionOutcome> = None;
    let exceeds_bytes = max_body > 0 && request_body.len() > max_body;
    let exceeds_tokens = max_tokens > 0 && (estimated_input_tokens.max(0) as usize) > max_tokens;
    if (exceeds_bytes || exceeds_tokens) && compression_config.enabled {
        // 自适应二次压缩：按 request_body_bytes 迭代截断，尽量把请求缩到阈值内
        match adaptive_shrink_request_body(
            &mut kiro_request,
            &compression_config,
            max_body,
            max_tokens,
            &mut request_body,
        ) {
            Ok(Some(outcome)) => {
                tracing::warn!(
                    conversation_id = kiro_request.conversation_state.conversation_id.as_str(),
                    trigger = if exceeds_tokens { "tokens" } else { "bytes" },
                    initial_bytes = outcome.initial_bytes,
                    final_bytes = outcome.final_bytes,
                    threshold = max_body,
                    estimated_input_tokens,
                    token_threshold = max_tokens,
                    iters = outcome.iters,
                    additional_history_turns_removed = outcome.additional_history_turns_removed,
                    final_tool_result_max_chars = outcome.final_tool_result_max_chars,
                    final_tool_use_input_max_chars = outcome.final_tool_use_input_max_chars,
                    final_message_content_max_chars = outcome.final_message_content_max_chars,
                    "请求体超过阈值，已执行自适应二次压缩"
                );
                adaptive_outcome = Some(outcome);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!("自适应二次压缩序列化失败: {}", e);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "internal_error",
                        format!("序列化请求失败: {}", e),
                    )),
                )
                    .into_response();
            }
        }
    }

    // 压缩后再次检查（输出 image_bytes/non-image bytes 便于排查）
    let final_img_bytes = total_image_bytes(&kiro_request);
    let final_effective_len = request_body.len().saturating_sub(final_img_bytes);
    if max_body > 0 && request_body.len() > max_body {
        tracing::warn!(
            conversation_id = kiro_request.conversation_state.conversation_id.as_str(),
            request_body_bytes = request_body.len(),
            image_bytes = final_img_bytes,
            effective_bytes = final_effective_len,
            threshold = max_body,
            "请求体超过安全阈值，拒绝发送"
        );
        #[cfg(feature = "sensitive-logs")]
        tracing::error!(
            "自适应压缩仍超限，完整请求体（用于诊断）: {}",
            truncate_base64_in_request_body(&request_body)
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new(
                "invalid_request_error",
                format!(
                    "Request too large ({} bytes total; images {} bytes; non-image {} bytes; limit {}). Reduce conversation history/tool output or number/size of images.",
                    request_body.len(),
                    final_img_bytes,
                    final_effective_len,
                    max_body
                ),
            )),
        )
            .into_response();
    }

    tracing::debug!(
        kiro_request_body_bytes = request_body.len(),
        "已构建 Kiro 请求体"
    );

    // 跨请求缓存插入
    if let (Some(cache), Some(fp)) = (&state.cross_request_cache, &content_fingerprint) {
        let conv_id = kiro_request.conversation_state.conversation_id.clone();
        cache.insert(*fp, conv_id);
        tracing::debug!("已缓存 conversation_id 到跨请求缓存");
    }

    // 检查是否启用了thinking
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .map(|t| t.is_enabled())
        .unwrap_or(false);

    if payload.stream {
        // 流式响应
        let stream_request = StreamRequestContext {
            cache_tracker: prompt_cache
                .accounting_enabled
                .then_some(&prompt_cache.tracker),
            cache_profile: cache_profile.as_ref(),
            request_body: &request_body,
            model: &payload.model,
            input_tokens: estimated_input_tokens,
            thinking_enabled,
            tool_name_map: tool_name_map.clone(),
            user_id: user_id.as_deref(),
            metrics: state.metrics.as_ref(),
            request_start,
            adaptive_outcome: adaptive_outcome.as_ref(),
            defer_message_start: uri.path().starts_with("/anthropic/"),
        };
        handle_stream_request(provider, stream_request, &state, &auth).await
    } else {
        // 非流式响应
        let non_stream_request = NonStreamRequestContext {
            request_body: &request_body,
            model: &payload.model,
            input_tokens: estimated_input_tokens,
            tool_name_map,
            user_id: user_id.as_deref(),
            cache_tracker: prompt_cache
                .accounting_enabled
                .then_some(&prompt_cache.tracker),
            cache_profile: cache_profile.as_ref(),
            metrics: state.metrics.as_ref(),
            request_start,
            adaptive_outcome: adaptive_outcome.as_ref(),
        };
        handle_non_stream_request(provider, non_stream_request, &state, &auth).await
    }
}
async fn handle_stream_request(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    context: StreamRequestContext<'_>,
    state: &AppState,
    auth: &AuthIdentity,
) -> Response {
    // 调用 Kiro API（支持多凭据故障转移）
    let api_result = match provider
        .call_api_stream(context.request_body, context.user_id, auth.group.as_deref())
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            let crate::kiro::provider::ApiCallError { error, attempts } = e;
            let error_message = error.to_string();
            let credential_id = last_attempt_credential_id(&attempts);
            let elapsed_ms = context.request_start.elapsed().as_millis() as u64;
            tracing::warn!(
                elapsed_ms = elapsed_ms,
                model = %context.model,
                "请求失败（流式，含重试耗时）"
            );
            record_request_telemetry(
                state,
                auth,
                TelemetryData {
                    model: context.model,
                    is_stream: true,
                    credential_id,
                    input_tokens: context.input_tokens,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    credits: 0.0,
                    duration_ms: elapsed_ms,
                    status: "error",
                    attempts,
                    first_token_ms: None,
                    error_message: Some(error_message),
                },
            );
            return map_kiro_provider_error_to_response(
                context.request_body,
                error,
                context.adaptive_outcome,
            );
        }
    };

    let final_cache_context = match (context.cache_tracker, context.cache_profile) {
        (Some(tracker), Some(profile)) => {
            let resolved = resolved_cache_usage(tracker, api_result.credential_id, profile);
            tracing::info!(
                credential_id = api_result.credential_id,
                final_cache_creation_input_tokens = resolved.cache_creation_input_tokens,
                final_cache_read_input_tokens = resolved.cache_read_input_tokens,
                "Resolved cache usage for stream request"
            );
            tracker.update(api_result.credential_id, profile);
            Some(resolved)
        }
        _ => None,
    };
    let final_cache_usage = final_cache_context.map(|ctx| CacheUsageBreakdown {
        cache_creation_input_tokens: ctx.cache_creation_input_tokens,
        cache_read_input_tokens: ctx.cache_read_input_tokens,
    });

    // 创建流处理上下文
    let mut ctx = StreamContext::new_with_thinking(
        context.model,
        context.input_tokens,
        final_cache_usage,
        context.thinking_enabled,
        context.tool_name_map,
    );

    // Claude Code 端点启用 defer 模式，等 contextUsageEvent 精确 input_tokens
    if context.defer_message_start {
        ctx.defer_message_start = true;
    }

    // 生成初始事件
    let initial_events = ctx.generate_initial_events();

    // 克隆 Arc 用于流式结束时的用量/追踪记录
    let stream_state = state.clone();
    let stream_auth = auth.clone();
    let stream_model = context.model.to_string();
    let stream_credential_id = api_result.credential_id;
    let stream_start = context.request_start;
    let stream_input_tokens = context.input_tokens;
    let stream_attempts = api_result.attempts;

    // 用于从 unfold closure 中提取流结束时的真实 token 用量
    let usage_snapshot = std::sync::Arc::new(parking_lot::Mutex::new(None::<StreamUsageSnapshot>));
    let usage_snapshot_for_stream = usage_snapshot.clone();

    // 创建 SSE 流
    let stream = create_sse_stream(
        api_result.response,
        ctx,
        initial_events,
        usage_snapshot_for_stream,
    );

    // 记录到首字节的延迟（在流开始前捕获，传入 closure）
    let ttfb_ms = context.request_start.elapsed().as_millis() as u64;

    // 在流结束后追加用量记录（从 snapshot 读取真实 token 计数）
    let stream = stream.chain(futures::stream::once(async move {
        let duration_ms = stream_start.elapsed().as_millis() as u64;
        let snap = usage_snapshot.lock().take().unwrap_or_default();
        let total_output = snap.output_tokens + snap.thinking_tokens;
        let billed_input = if snap.input_tokens > 0 {
            snap.input_tokens
        } else {
            billed_input_tokens(stream_input_tokens, snap.cache_creation, snap.cache_read)
        };
        record_request_telemetry(
            &stream_state,
            &stream_auth,
            TelemetryData {
                model: &stream_model,
                is_stream: true,
                credential_id: stream_credential_id,
                input_tokens: billed_input,
                output_tokens: total_output,
                cache_creation_tokens: snap.cache_creation,
                cache_read_tokens: snap.cache_read,
                credits: snap.credits,
                duration_ms,
                status: "success",
                attempts: stream_attempts,
                first_token_ms: Some(ttfb_ms),
                error_message: None,
            },
        );
        Ok(Bytes::new())
    }));
    tracing::info!(
        ttfb_ms = ttfb_ms,
        credential_id = api_result.credential_id,
        model = %context.model,
        "请求完成（流式 TTFB）"
    );
    if let Some(metrics) = context.metrics {
        metrics.record(
            MetricEvent::new(MetricEventType::RequestCompleted)
                .with_model(context.model)
                .with_status("success")
                .with_latency_ms(ttfb_ms)
                .with_tokens(context.input_tokens, 0),
        );
    }

    // 返回 SSE 响应（含 Anthropic 标准 headers）
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive");
    for (key, value) in anthropic_response_headers().iter() {
        builder = builder.header(key, value);
    }
    builder.body(Body::from_stream(stream)).unwrap()
}

/// Ping 事件间隔（25秒）
const PING_INTERVAL_SECS: u64 = 25;

/// 创建 ping 事件的 SSE 字符串
fn create_ping_sse() -> Bytes {
    Bytes::from("event: ping\ndata: {\"type\": \"ping\"}\n\n")
}

/// 创建 SSE 事件流
fn create_sse_stream(
    response: reqwest::Response,
    ctx: StreamContext,
    initial_events: Vec<SseEvent>,
    usage_snapshot: std::sync::Arc<parking_lot::Mutex<Option<StreamUsageSnapshot>>>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    // 先发送初始事件
    let initial_stream = stream::iter(
        initial_events
            .into_iter()
            .map(|e| Ok(Bytes::from(e.to_sse_string()))),
    );

    // 然后处理 Kiro 响应流，同时每25秒发送 ping 保活
    let body_stream = response.bytes_stream();
    let ping_period = Duration::from_secs(PING_INTERVAL_SECS);
    let ping_interval = interval_at(Instant::now() + ping_period, ping_period);

    let processing_stream = stream::unfold(
        (body_stream, ctx, EventStreamDecoder::new(), false, ping_interval, usage_snapshot),
        |(mut body_stream, mut ctx, mut decoder, finished, mut ping_interval, usage_snapshot)| async move {
            if finished {
                return None;
            }

            // 使用 select! 同时等待数据和 ping 定时器
            tokio::select! {
                // 处理数据流
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            // 解码事件
                            if let Err(e) = decoder.feed(&chunk) {
                                tracing::warn!("缓冲区溢出: {}", e);
                            }

                            let mut events = Vec::new();
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => {
                                        if let Ok(event) = Event::from_frame(frame) {
                                            let sse_events = ctx.process_kiro_event(&event);
                                            events.extend(sse_events);
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("解码事件失败: {}", e);
                                    }
                                }
                            }

                            // 转换为 SSE 字节流
                            let bytes: Vec<Result<Bytes, Infallible>> = events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();

                            Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval, usage_snapshot)))
                        }
                        Some(Err(e)) => {
                            tracing::error!("读取响应流失败: {}", e);
                            // 上游流中断 → 标记 stop_reason="error"。否则
                            // generate_final_events() 走默认 end_turn，客户端
                            // 会把不完整 assistant turn 当成正常结束写回 history。
                            ctx.state_manager.set_stop_reason("error");
                            let final_events = ctx.generate_final_events();
                            // 写入 usage snapshot 供 handler 层 telemetry 使用
                            let telemetry_input_tokens = stream_telemetry_input_tokens(&ctx);
                            *usage_snapshot.lock() = Some(StreamUsageSnapshot {
                                input_tokens: telemetry_input_tokens,
                                output_tokens: ctx.output_tokens,
                                thinking_tokens: ctx.thinking_tokens,
                                cache_creation: ctx.cache_usage.map(|c| c.cache_creation_input_tokens).unwrap_or(0),
                                cache_read: ctx.cache_usage.map(|c| c.cache_read_input_tokens).unwrap_or(0),
                                credits: ctx.metering.as_ref().map(|m| m.usage).unwrap_or(0.0),
                            });
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, usage_snapshot)))
                        }
                        None => {
                            // 流结束，发送最终事件
                            let final_events = ctx.generate_final_events();
                            // 写入 usage snapshot 供 handler 层 telemetry 使用
                            let telemetry_input_tokens = stream_telemetry_input_tokens(&ctx);
                            *usage_snapshot.lock() = Some(StreamUsageSnapshot {
                                input_tokens: telemetry_input_tokens,
                                output_tokens: ctx.output_tokens,
                                thinking_tokens: ctx.thinking_tokens,
                                cache_creation: ctx.cache_usage.map(|c| c.cache_creation_input_tokens).unwrap_or(0),
                                cache_read: ctx.cache_usage.map(|c| c.cache_read_input_tokens).unwrap_or(0),
                                credits: ctx.metering.as_ref().map(|m| m.usage).unwrap_or(0.0),
                            });
                            let bytes: Vec<Result<Bytes, Infallible>> = final_events
                                .into_iter()
                                .map(|e| Ok(Bytes::from(e.to_sse_string())))
                                .collect();
                            Some((stream::iter(bytes), (body_stream, ctx, decoder, true, ping_interval, usage_snapshot)))
                        }
                    }
                }
                // 发送 ping 保活
                _ = ping_interval.tick() => {
                    tracing::trace!("发送 ping 保活事件");
                    let bytes: Vec<Result<Bytes, Infallible>> = vec![Ok(create_ping_sse())];
                    Some((stream::iter(bytes), (body_stream, ctx, decoder, false, ping_interval, usage_snapshot)))
                }
            }
        },
    )
    .flatten();

    initial_stream.chain(processing_stream)
}

/// 处理非流式请求
async fn handle_non_stream_request(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    context: NonStreamRequestContext<'_>,
    state: &AppState,
    auth: &AuthIdentity,
) -> Response {
    // 调用 Kiro API（支持多凭据故障转移）
    let api_result = match provider
        .call_api(context.request_body, context.user_id, auth.group.as_deref())
        .await
    {
        Ok(resp) => resp,
        Err(e) => {
            let crate::kiro::provider::ApiCallError { error, attempts } = e;
            let error_message = error.to_string();
            let credential_id = last_attempt_credential_id(&attempts);
            let elapsed_ms = context.request_start.elapsed().as_millis() as u64;
            tracing::warn!(
                elapsed_ms = elapsed_ms,
                model = %context.model,
                "请求失败（非流式，含重试耗时）"
            );
            record_request_telemetry(
                state,
                auth,
                TelemetryData {
                    model: context.model,
                    is_stream: false,
                    credential_id,
                    input_tokens: context.input_tokens,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    credits: 0.0,
                    duration_ms: elapsed_ms,
                    status: "error",
                    attempts,
                    first_token_ms: None,
                    error_message: Some(error_message),
                },
            );
            return map_kiro_provider_error_to_response(
                context.request_body,
                error,
                context.adaptive_outcome,
            );
        }
    };

    // 非流式 TTFB：provider 返回（HTTP 响应头到达）到请求开始的时间
    let ttfb_ms = context.request_start.elapsed().as_millis() as u64;

    let final_cache_context = match (context.cache_tracker, context.cache_profile) {
        (Some(tracker), Some(profile)) => {
            let resolved = resolved_cache_usage(tracker, api_result.credential_id, profile);
            tracing::info!(
                credential_id = api_result.credential_id,
                final_cache_creation_input_tokens = resolved.cache_creation_input_tokens,
                final_cache_read_input_tokens = resolved.cache_read_input_tokens,
                "Resolved cache usage for non-stream request"
            );
            tracker.update(api_result.credential_id, profile);
            Some(resolved)
        }
        _ => None,
    };

    // 读取响应体
    let body_bytes = match api_result.response.bytes().await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("读取响应体失败: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    format!("读取响应失败: {}", e),
                )),
            )
                .into_response();
        }
    };

    // === T3 空 tool_result 判别循环 ===
    // 上游 Kiro 偶发在收到 tool_result 后返回只含 thinking 无可见文本的空洞回合，
    // 直接序列化为 end_turn 会让 Codex 等下游误以为任务完成。此处仅在最后一轮
    // user 消息含 toolResults 且回合确实"空洞"时重试一次；第二次仍空洞返回 502。
    let is_tool_result_turn = request_is_tool_result_continuation(context.request_body);
    let mut retry_used = false;
    let mut current_body_bytes = body_bytes;
    let mut current_api_result_credential_id = api_result.credential_id;
    let mut current_api_result_attempts = api_result.attempts.clone();

    loop {
        // 解析事件流
        let mut decoder = EventStreamDecoder::new();
        if let Err(e) = decoder.feed(&current_body_bytes) {
            tracing::warn!("缓冲区溢出: {}", e);
        }

        let mut text_content = String::new();
        // reasoningContentEvent 累积（thinking 模型独立推理流），最终作为 thinking content block 返回
        let mut reasoning_text = String::new();
        let mut reasoning_signature: Option<String> = None;
        let mut tool_uses: Vec<serde_json::Value> = Vec::new();
        let mut has_tool_use = false;
        let mut stop_reason = "end_turn".to_string();
        #[cfg(feature = "sensitive-logs")]
        let mut context_input_tokens_for_log: Option<i32> = None;
        // 从 meteringEvent 透传的 credit usage，仅用于最终 usage 字段
        let mut metering: Option<MeteringEvent> = None;

        // 收集工具调用的增量 JSON
        let mut tool_json_buffers: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        for result in decoder.decode_iter() {
            match result {
                Ok(frame) => {
                    if let Ok(event) = Event::from_frame(frame) {
                        match event {
                            Event::AssistantResponse(resp) => {
                                text_content.push_str(&resp.content);
                            }
                            Event::ReasoningContent(reasoning) => {
                                reasoning_text.push_str(&reasoning.text);
                                if let Some(sig) = reasoning.signature {
                                    reasoning_signature = Some(sig);
                                }
                            }
                            Event::ToolUse(tool_use) => {
                                has_tool_use = true;

                                // 累积工具的 JSON 输入
                                let buffer = tool_json_buffers
                                    .entry(tool_use.tool_use_id.clone())
                                    .or_default();
                                buffer.push_str(&tool_use.input);

                                // 如果是完整的工具调用，添加到列表
                                if tool_use.stop {
                                    let input: serde_json::Value = if buffer.trim().is_empty() {
                                        // 上游可能省略无参工具的 input 字段（或传空字符串）。
                                        // 这里将其视为合法的空对象，避免 EOF 解析错误导致日志噪音。
                                        serde_json::json!({})
                                    } else {
                                        serde_json::from_str(buffer).unwrap_or_else(|e| {
                                        // 检测是否为截断导致的解析失败
                                        if let Some(truncation_info) =
                                            super::truncation::detect_truncation(
                                                &tool_use.name,
                                                &tool_use.tool_use_id,
                                                buffer,
                                            )
                                        {
                                            let soft_msg =
                                                super::truncation::build_soft_failure_result(
                                                    &truncation_info,
                                                );
                                            tracing::warn!(
                                                tool_use_id = %tool_use.tool_use_id,
                                                truncation_type = %truncation_info.truncation_type,
                                                "检测到工具调用截断: {}", soft_msg
                                            );
                                        }

                                        // 仅在显式开启敏感日志时输出完整内容
                                        #[cfg(feature = "sensitive-logs")]
                                        tracing::warn!(
                                            tool_use_id = %tool_use.tool_use_id,
                                            buffer = %buffer,
                                            request_body = %truncate_middle(context.request_body, 1200),
                                            "工具输入 JSON 解析失败: {e}"
                                        );
                                        #[cfg(not(feature = "sensitive-logs"))]
                                        tracing::warn!(
                                            tool_use_id = %tool_use.tool_use_id,
                                            buffer_bytes = buffer.len(),
                                            request_body_bytes = context.request_body.len(),
                                            "工具输入 JSON 解析失败: {e}"
                                        );
                                        serde_json::json!({})
                                    })
                                    };

                                    // 释放已完成的 buffer，避免请求处理期间内存重复占用
                                    tool_json_buffers.remove(&tool_use.tool_use_id);

                                    let original_name = context
                                        .tool_name_map
                                        .get(&tool_use.name)
                                        .cloned()
                                        .unwrap_or_else(|| tool_use.name.clone());

                                    tool_uses.push(json!({
                                        "type": "tool_use",
                                        "id": tool_use.tool_use_id,
                                        "name": original_name,
                                        "input": input
                                    }));
                                }
                            }
                            Event::ContextUsage(context_usage) => {
                                // 从上下文使用百分比计算实际的 input_tokens
                                let context_window =
                                    super::types::get_context_window_size(context.model) as f64;
                                let actual_input_tokens =
                                    (context_usage.context_usage_percentage * context_window
                                        / 100.0) as i32;
                                #[cfg(feature = "sensitive-logs")]
                                {
                                    context_input_tokens_for_log = Some(actual_input_tokens);
                                }
                                // 上下文使用量达到 100% 时，设置 stop_reason 为 model_context_window_exceeded
                                if context_usage.context_usage_percentage >= 100.0 {
                                    stop_reason = "model_context_window_exceeded".to_string();
                                }
                                tracing::debug!(
                                    "收到 contextUsageEvent: {}%, 计算 input_tokens: {} (context_window: {})",
                                    context_usage.context_usage_percentage,
                                    actual_input_tokens,
                                    context_window as i32
                                );
                            }
                            Event::Metering(event_metering) => {
                                tracing::debug!(
                                    usage = event_metering.usage,
                                    unit = %event_metering.unit,
                                    unit_plural = %event_metering.unit_plural,
                                    "收到 meteringEvent"
                                );
                                metering = Some(event_metering);
                            }
                            Event::Exception { exception_type, .. }
                                if exception_type == "ContentLengthExceededException" =>
                            {
                                stop_reason = "max_tokens".to_string();
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("解码事件失败: {}", e);
                }
            }
        }

        // 确定 stop_reason
        if has_tool_use && stop_reason == "end_turn" {
            stop_reason = "tool_use".to_string();
        }

        // 提前提取 thinking 判断"空洞"用（真正 content 组装在 loop 外）
        let (probe_extracted_thinking, probe_cleaned_text) = extract_thinking_xml(&text_content);

        // T3 空洞检测：只有"工具结果继续 + 无可见文本 + 无工具调用 + end_turn"才判定
        if is_tool_result_turn
            && is_empty_assistant_turn(&probe_cleaned_text, tool_uses.len(), &stop_reason)
        {
            if !retry_used {
                retry_used = true;
                tracing::warn!(
                    model = %context.model,
                    credential_id = current_api_result_credential_id,
                    had_thinking = %(!probe_extracted_thinking.is_empty() || !reasoning_text.is_empty()),
                    "检测到工具结果后空洞回合，触发一次重试（丢弃当前 thinking，避免复读）"
                );
                // 重发同一请求；使用同一凭据池策略。失败或再次空洞时返回 502。
                match provider
                    .call_api(context.request_body, context.user_id, auth.group.as_deref())
                    .await
                {
                    Ok(retry_result) => {
                        let retry_credential_id = retry_result.credential_id;
                        let retry_attempts = retry_result.attempts.clone();
                        let retry_bytes = match retry_result.response.bytes().await {
                            Ok(b) => b,
                            Err(e) => {
                                tracing::error!("空洞回合重试读取响应体失败: {}", e);
                                return (
                                    StatusCode::BAD_GATEWAY,
                                    Json(ErrorResponse::new(
                                        "api_error",
                                        "上游返回空洞回合且重试读取失败".to_string(),
                                    )),
                                )
                                    .into_response();
                            }
                        };
                        current_body_bytes = retry_bytes;
                        // 更新 current_api_result 用于后续 telemetry；response 字段本身已被消费，
                        // 后续只用 credential_id / attempts，不再触碰 response。
                        current_api_result_credential_id = retry_credential_id;
                        current_api_result_attempts = retry_attempts;
                        continue;
                    }
                    Err(e) => {
                        let crate::kiro::provider::ApiCallError { error, .. } = e;
                        tracing::warn!(error = %error, "空洞回合重试请求失败");
                        return (
                            StatusCode::BAD_GATEWAY,
                            Json(ErrorResponse::new(
                                "api_error",
                                "上游返回空洞回合且重试失败".to_string(),
                            )),
                        )
                            .into_response();
                    }
                }
            } else {
                tracing::warn!(
                    model = %context.model,
                    credential_id = current_api_result_credential_id,
                    "工具结果继续场景重试后仍为空洞回合，返回 502"
                );
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(ErrorResponse::new(
                        "api_error",
                        "上游连续返回空洞回合（tool_result 继续场景）".to_string(),
                    )),
                )
                    .into_response();
            }
        }

        // 非空洞回合：跳出重试循环，继续走响应组装
        // 构建响应内容
        let mut content: Vec<serde_json::Value> = Vec::new();

        // 实测发现：Q 上游对**非流式**请求不发独立 reasoningContentEvent，
        // 而是把推理以 `<thinking>...</thinking>` XML 标签嵌入 assistantResponse 文本。
        // 这里从 text_content 提取 thinking 标签升级为独立 thinking content_block，
        // 跟流式 reasoningContentEvent 路径保持响应结构一致。
        // 若没有 XML 标签但有 reasoning_text（流式情况下被聚合走非流式 handler 的边缘场景），
        // 直接用 reasoning_text。
        let cleaned_text = probe_cleaned_text;
        let extracted_thinking = probe_extracted_thinking;
        let final_thinking = if !reasoning_text.is_empty() {
            reasoning_text
        } else {
            extracted_thinking
        };

        // thinking 块必须排在 text/tool 之前（Anthropic 协议要求）。
        // signature 字段必须存在（空字符串可被客户端 SDK 接受，但有 signature 才能写回 history）。
        if !final_thinking.is_empty() {
            content.push(json!({
                "type": "thinking",
                "thinking": final_thinking,
                "signature": reasoning_signature.unwrap_or_default(),
            }));
        }

        if !cleaned_text.is_empty() {
            content.push(json!({
                "type": "text",
                "text": cleaned_text
            }));
        }

        content.extend(tool_uses);

        // 估算输出 tokens
        let output_tokens = token::estimate_output_tokens(&content);

        // non-stream 与 stream 保持一致：始终使用本地估算的 input_tokens 作为最终口径。
        let final_input_tokens = context.input_tokens;
        let billed_input_tokens = final_cache_context
            .map(|ctx| {
                billed_input_tokens(
                    final_input_tokens,
                    ctx.cache_creation_input_tokens,
                    ctx.cache_read_input_tokens,
                )
            })
            .unwrap_or(final_input_tokens);

        #[cfg(feature = "sensitive-logs")]
        tracing::info!(
            estimated_input_tokens = context.input_tokens,
            context_input_tokens = ?context_input_tokens_for_log,
            final_input_tokens,
            billed_input_tokens,
            output_tokens,
            "Non-stream usage: final_input_tokens={} (估算值), context_input_tokens={} (上游值), billed_input_tokens={}, output_tokens={}",
            final_input_tokens,
            context_input_tokens_for_log.map_or("N/A".to_string(), |v| v.to_string()),
            billed_input_tokens,
            output_tokens
        );

        let response_body = {
            let mut usage = json!({
                "input_tokens": billed_input_tokens,
                "output_tokens": output_tokens
            });
            if let Some(ref metering) = metering {
                inject_credit_usage_fields(&mut usage, metering);
            }
            if let Some(cache_context) = final_cache_context {
                inject_cache_usage_fields(&mut usage, cache_context);
            }

            json!({
                "id": format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
                "type": "message",
                "role": "assistant",
                "content": content,
                "model": context.model,
                "stop_reason": stop_reason,
                "stop_sequence": null,
                "usage": usage
            })
        };

        // 记录 RequestCompleted 指标（非流式：完整请求延迟 + token 统计）
        let total_ms = context.request_start.elapsed().as_millis() as u64;
        tracing::info!(
            ttfb_ms = ttfb_ms,
            total_ms = total_ms,
            credential_id = current_api_result_credential_id,
            model = %context.model,
            output_tokens = output_tokens,
            "请求完成（非流式）"
        );
        if let Some(metrics) = context.metrics {
            metrics.record(
                MetricEvent::new(MetricEventType::RequestCompleted)
                    .with_model(context.model)
                    .with_status("success")
                    .with_latency_ms(total_ms)
                    .with_tokens(context.input_tokens, output_tokens),
            );
        }

        let credits = metering.as_ref().map(|m| m.usage).unwrap_or(0.0);
        record_request_telemetry(
            state,
            auth,
            TelemetryData {
                model: context.model,
                is_stream: false,
                credential_id: current_api_result_credential_id,
                input_tokens: billed_input_tokens,
                output_tokens,
                cache_creation_tokens: final_cache_context
                    .map(|c| c.cache_creation_input_tokens)
                    .unwrap_or(0),
                cache_read_tokens: final_cache_context
                    .map(|c| c.cache_read_input_tokens)
                    .unwrap_or(0),
                credits,
                duration_ms: total_ms,
                status: "success",
                attempts: std::mem::take(&mut current_api_result_attempts),
                first_token_ms: Some(ttfb_ms),
                error_message: None,
            },
        );

        break (
            StatusCode::OK,
            anthropic_response_headers(),
            Json(response_body),
        )
            .into_response();
    } // end of T3 retry loop
}

/// 检测模型名是否包含 "thinking" 后缀，若包含则覆写 thinking 配置
///
/// 支持的后缀格式：
/// - `-thinking-minimal` → budget 512
/// - `-thinking-low` → budget 1024
/// - `-thinking-medium` → budget 8192
/// - `-thinking-high` → budget 24576
/// - `-thinking-xhigh` → budget 32768
/// - `-thinking` → budget 20000（默认）
///
/// - Opus 4.6：覆写为 adaptive 类型
/// - 其他模型：覆写为 enabled 类型
fn override_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model_lower = payload.model.to_lowercase();
    if !model_lower.contains("thinking") {
        return;
    }

    // 具体后缀必须在通用 "thinking" 之前匹配
    let budget_tokens = if model_lower.ends_with("-thinking-minimal") {
        512
    } else if model_lower.ends_with("-thinking-low") {
        1024
    } else if model_lower.ends_with("-thinking-medium") {
        8192
    } else if model_lower.ends_with("-thinking-high") {
        24576
    } else if model_lower.ends_with("-thinking-xhigh") {
        32768
    } else if model_lower.ends_with("-thinking") {
        20000
    } else {
        // "thinking" 出现在模型名中但不是后缀（如 "thinking-model-v2"），不覆写
        return;
    };

    let is_opus_or_sonnet_4_6 = (model_lower.contains("opus") || model_lower.contains("sonnet"))
        && (model_lower.contains("4-6") || model_lower.contains("4.6"));

    let thinking_type = if is_opus_or_sonnet_4_6 {
        "adaptive"
    } else {
        "enabled"
    };

    tracing::info!(
        model = %payload.model,
        thinking_type = thinking_type,
        budget_tokens = budget_tokens,
        "模型名包含 thinking 后缀，覆写 thinking 配置"
    );

    payload.thinking = Some(Thinking {
        thinking_type: thinking_type.to_string(),
        budget_tokens,
    });

    if is_opus_or_sonnet_4_6 {
        payload.output_config = Some(OutputConfig {
            effort: "high".to_string(),
        });
    }
}

/// POST /v1/messages/count_tokens
///
/// 计算消息的 token 数量。
pub async fn count_tokens(
    OriginalUri(uri): OriginalUri,
    JsonExtractor(payload): JsonExtractor<CountTokensRequest>,
) -> impl IntoResponse {
    tracing::info!(
        path = %uri.path(),
        model = %payload.model,
        message_count = %payload.messages.len(),
        "Received request"
    );

    let total_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;

    Json(CountTokensResponse {
        input_tokens: total_tokens.max(1),
    })
}

/// 截断字符串中间部分，保留头尾各 `keep` 个字符
///
/// 用于 debug 日志：避免输出过长的请求体，同时保留足够上下文便于排查。
/// 正确处理 UTF-8 多字节字符边界，不会截断中文。
#[cfg(feature = "sensitive-logs")]
fn truncate_middle(s: &str, keep: usize) -> std::borrow::Cow<'_, str> {
    // 按字符数计算，避免截断后反而更长
    let char_count = s.chars().count();
    let min_omit = 30; // 省略号 + 数字的最小开销，确保截断有意义
    if char_count <= keep * 2 + min_omit {
        return std::borrow::Cow::Borrowed(s);
    }

    // 找到第 keep 个字符的字节边界
    let head_end = s
        .char_indices()
        .nth(keep)
        .map(|(i, _)| i)
        .unwrap_or(s.len());

    // 找到倒数第 keep 个字符的字节边界
    let tail_start = s
        .char_indices()
        .nth_back(keep - 1)
        .map(|(i, _)| i)
        .unwrap_or(0);

    let omitted = s.len() - head_end - (s.len() - tail_start);
    std::borrow::Cow::Owned(format!(
        "{}...({} bytes omitted)...{}",
        &s[..head_end],
        omitted,
        &s[tail_start..]
    ))
}

/// sensitive-logs 模式下输出完整请求体，但截断 base64 图片数据
///
/// 图片 base64 数据对诊断 400 错误没有价值，但可能占几十 KB。
/// 扫描 `"bytes":"<base64...>"` 模式，将长 base64 替换为占位符。
#[cfg(feature = "sensitive-logs")]
fn truncate_base64_in_request_body(s: &str) -> std::borrow::Cow<'_, str> {
    const MARKER: &str = r#""bytes":""#;
    const MIN_BASE64_LEN: usize = 200;

    // 快速路径：没有 "bytes":" 就直接返回
    if !s.contains(MARKER) {
        return std::borrow::Cow::Borrowed(s);
    }

    let mut result = String::with_capacity(s.len());
    let mut pos = 0;
    let bytes = s.as_bytes();

    while pos < bytes.len() {
        if let Some(offset) = s[pos..].find(MARKER) {
            let marker_start = pos + offset;
            let value_start = marker_start + MARKER.len();

            // 找到闭合引号（处理转义）
            let mut end = value_start;
            let mut escaped = false;
            while end < bytes.len() {
                if escaped {
                    escaped = false;
                    end += 1;
                    continue;
                }
                match bytes[end] {
                    b'\\' => {
                        escaped = true;
                        end += 1;
                    }
                    b'"' => break,
                    _ => end += 1,
                }
            }

            let value_len = end - value_start;
            if value_len >= MIN_BASE64_LEN && is_likely_base64(&s[value_start..end]) {
                result.push_str(&s[pos..value_start]);
                result.push_str(&format!("<BASE64_TRUNCATED:{}>", value_len));
                pos = end; // 跳到闭合引号，下一轮会输出它
            } else {
                // 不是 base64 或太短，原样保留
                result.push_str(&s[pos..value_start]);
                pos = value_start;
            }
        } else {
            result.push_str(&s[pos..]);
            break;
        }
    }

    std::borrow::Cow::Owned(result)
}

#[cfg(feature = "sensitive-logs")]
fn is_likely_base64(s: &str) -> bool {
    s.bytes()
        .take(100)
        .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

#[cfg(test)]
mod empty_tool_result_tests {
    use super::*;

    #[test]
    fn empty_turn_only_when_all_conditions_meet() {
        assert!(is_empty_assistant_turn("", 0, "end_turn"));
        assert!(is_empty_assistant_turn("   \n\t  ", 0, "end_turn"));
        // 有文本 → 非空洞
        assert!(!is_empty_assistant_turn("hello", 0, "end_turn"));
        // 有工具调用 → 非空洞
        assert!(!is_empty_assistant_turn("", 1, "end_turn"));
        // 非 end_turn → 非空洞（tool_use / max_tokens 等）
        assert!(!is_empty_assistant_turn("", 0, "tool_use"));
        assert!(!is_empty_assistant_turn("", 0, "max_tokens"));
    }

    #[test]
    fn request_probe_detects_tool_result_continuation() {
        let with = r#"{
            "conversationState": {
                "currentMessage": {
                    "userInputMessage": {
                        "userInputMessageContext": {
                            "toolResults": [{"toolUseId":"x","content":[],"status":"success"}]
                        }
                    }
                }
            }
        }"#;
        assert!(request_is_tool_result_continuation(with));

        let empty = r#"{"conversationState":{"currentMessage":{"userInputMessage":{"userInputMessageContext":{"toolResults":[]}}}}}"#;
        assert!(!request_is_tool_result_continuation(empty));

        let missing = r#"{"conversationState":{"currentMessage":{"userInputMessage":{"userInputMessageContext":{}}}}}"#;
        assert!(!request_is_tool_result_continuation(missing));

        assert!(!request_is_tool_result_continuation("not json"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::trace_db::{TraceAttempt, TraceKeySource, TraceQuery, TraceStore, outcome};
    use crate::anthropic::types::{Message, SystemMessage};
    use crate::kiro::model::credentials::KiroCredentials;
    use crate::kiro::model::requests::conversation::{
        ConversationState, CurrentMessage, KiroImage, Message as KiroMessage, UserInputMessage,
    };
    use crate::kiro::provider::KiroProvider;
    use crate::kiro::token_manager::MultiTokenManager;
    use crate::model::config::Config;
    use std::sync::Arc;

    fn sample_messages_request() -> MessagesRequest {
        // 生成一个超过 1024 tokens 的 system message 用于测试缓存
        let long_text = "This is a test system message. ".repeat(100); // 约 600 tokens
        let very_long_text = format!("{}{}", long_text, long_text); // 约 1200 tokens

        MessagesRequest {
            model: "claude-sonnet-4-thinking".to_string(),
            max_tokens: 1024,
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type": "text", "text": "hello raw"},
                        {"type": "text", "text": ""}
                    ]),
                },
                Message {
                    role: "assistant".to_string(),
                    content: serde_json::json!("prefill that convert will drop"),
                },
            ],
            stream: false,
            system: Some(vec![SystemMessage {
                text: very_long_text,
                block_type: Some("text".to_string()),
                cache_control: Some(crate::anthropic::types::CacheControl {
                    cache_type: "ephemeral".to_string(),
                    ttl: None,
                }),
            }]),
            tools: Some(vec![crate::anthropic::types::Tool {
                tool_type: Some("web_search_20250305".to_string()),
                name: "web_search".to_string(),
                description: "search web".to_string(),
                input_schema: std::collections::HashMap::new(),
                max_uses: Some(1),
                cache_control: None,
                function: None,
            }]),
            tool_choice: None,
            thinking: None,
            output_config: None,
            reasoning_effort: None,
            metadata: None,
        }
    }

    #[test]
    fn test_cache_context_uses_raw_system_tokens() {
        let payload = sample_messages_request();

        let cache_tracker = crate::anthropic::cache_tracker::CacheTracker::new(
            std::time::Duration::from_secs(300),
            None,
        );

        // 计算实际的 system message tokens
        let system_text = &payload.system.as_ref().unwrap()[0].text;
        let expected = token::count_tokens(system_text) as i32;

        let cache_profile = build_cache_profile(&cache_tracker, &payload, expected);
        let cache_context = compute_cache_usage(&cache_tracker, 0, &cache_profile);

        // 验证 cache_creation_input_tokens 等于 system message 的 token 数
        assert_eq!(cache_context.cache_creation_input_tokens, expected);
        assert_eq!(cache_context.cache_read_input_tokens, 0);
    }

    #[test]
    fn test_resolved_cache_usage_uses_real_credential_id() {
        let payload = sample_messages_request();
        let estimated = token::count_all_tokens(
            payload.model.clone(),
            payload.system.clone(),
            payload.messages.clone(),
            payload.tools.clone(),
        ) as i32;
        let cache_tracker = crate::anthropic::cache_tracker::CacheTracker::new(
            std::time::Duration::from_secs(300),
            None,
        );
        let cache_profile = build_cache_profile(&cache_tracker, &payload, estimated);

        let provisional = provisional_cache_usage(&cache_tracker, &cache_profile);
        assert_eq!(provisional.cache_read_input_tokens, 0);

        cache_tracker.update(42, &cache_profile);
        let resolved = resolved_cache_usage(&cache_tracker, 42, &cache_profile);

        assert!(resolved.cache_read_input_tokens > 0);
        assert!(resolved.cache_creation_input_tokens <= provisional.cache_creation_input_tokens);
    }

    #[test]
    fn test_billed_input_tokens_subtracts_cache_tokens() {
        assert_eq!(billed_input_tokens(3829, 0, 1788), 2041);
        assert_eq!(billed_input_tokens(4131, 544, 2544), 1043);
        assert_eq!(billed_input_tokens(10, 3, 20), 0);
    }

    #[test]
    fn test_stream_telemetry_input_tokens_uses_billed_cache_breakdown() {
        let ctx = StreamContext::new_with_thinking(
            "test-model",
            100,
            Some(CacheUsageBreakdown {
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 95,
            }),
            false,
            std::collections::HashMap::new(),
        );

        assert_eq!(stream_telemetry_input_tokens(&ctx), 5);
    }

    #[test]
    fn test_non_stream_usage_uses_estimated_input_tokens_as_base() {
        let estimated_input_tokens = 1493;
        let upstream_context_input_tokens = 3106;
        let cache_creation_input_tokens = 9;
        let cache_read_input_tokens = 1480;

        let final_input_tokens = estimated_input_tokens;
        let billed = billed_input_tokens(
            final_input_tokens,
            cache_creation_input_tokens,
            cache_read_input_tokens,
        );

        assert_eq!(final_input_tokens, 1493);
        assert_eq!(upstream_context_input_tokens, 3106);
        assert_eq!(billed, 4);
        assert_ne!(final_input_tokens, upstream_context_input_tokens);
    }

    #[test]
    fn test_inject_cache_usage_fields_standard_only() {
        let mut usage = serde_json::json!({
            "input_tokens": 123,
            "output_tokens": 45
        });

        inject_cache_usage_fields(
            &mut usage,
            CacheUsageContext {
                cache_creation_input_tokens: 7,
                cache_read_input_tokens: 8,
                cache_creation_5m_input_tokens: 3,
                cache_creation_1h_input_tokens: 4,
            },
        );

        assert_eq!(usage["cache_creation_input_tokens"], 7);
        assert_eq!(usage["cache_read_input_tokens"], 8);
        // cache_creation 嵌套对象不再注入（非 Anthropic 标准）
        assert!(usage.get("cache_creation").is_none());
    }

    #[test]
    fn test_inject_credit_usage_fields_no_longer_injected() {
        let mut usage = serde_json::json!({
            "input_tokens": 123,
            "output_tokens": 45
        });

        inject_credit_usage_fields(
            &mut usage,
            &MeteringEvent {
                unit: "credit".to_string(),
                unit_plural: "credits".to_string(),
                usage: 0.5,
            },
        );

        // credit 字段不再注入到标准 usage
        assert!(usage.get("credit_usage").is_none());
        assert!(usage.get("credit_unit").is_none());
    }

    #[test]
    fn test_error_telemetry_preserves_provider_attempts_for_failure_stats() {
        let db_path =
            std::env::temp_dir().join(format!("kiro-rs-trace-regression-{}.db", Uuid::new_v4()));
        let store = Arc::new(TraceStore::open(db_path.clone(), true, 7).unwrap());
        let state = AppState::new(
            "test-api-key",
            Arc::new(parking_lot::RwLock::new(
                crate::anthropic::middleware::PromptCacheRuntime::new(300, false, None),
            )),
        )
        .with_trace_store(store.clone());
        let auth = AuthIdentity {
            key_id: 7,
            key_source: TraceKeySource::ClientKey,
            group: None,
        };

        record_request_telemetry(
            &state,
            &auth,
            TelemetryData {
                model: "test-model",
                is_stream: false,
                credential_id: 42,
                input_tokens: 10,
                output_tokens: 0,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                credits: 0.0,
                duration_ms: 25,
                status: "error",
                attempts: vec![TraceAttempt {
                    attempt: 0,
                    credential_id: 42,
                    endpoint: "ide".to_string(),
                    http_status: Some(403),
                    outcome: outcome::AUTH_FAILED.to_string(),
                    error_snippet: Some("forbidden".to_string()),
                    duration_ms: 25,
                }],
                first_token_ms: None,
                error_message: Some("auth failed after retries".to_string()),
            },
        );

        let stats = store.failure_stats();
        assert_eq!(stats.get(&42).map(|s| s.auth), Some(1));

        let (records, total) = store.query_paged(&TraceQuery {
            status: Some("error".to_string()),
            limit: 10,
            ..Default::default()
        });
        assert_eq!(total, 1);
        assert_eq!(records[0].final_credential_id, 42);
        assert_eq!(
            records[0].error_message.as_deref(),
            Some("auth failed after retries")
        );
        assert_eq!(records[0].attempts.len(), 1);
        assert_eq!(records[0].attempts[0].credential_id, 42);

        drop(state);
        drop(store);
        let _ = std::fs::remove_file(&db_path);
        let _ = std::fs::remove_file(db_path.with_file_name(format!(
            "{}-wal",
            db_path.file_name().unwrap().to_string_lossy()
        )));
        let _ = std::fs::remove_file(db_path.with_file_name(format!(
            "{}-shm",
            db_path.file_name().unwrap().to_string_lossy()
        )));
    }

    #[test]
    fn test_models_endpoint_includes_runtime_gateway_models() {
        let mut models = Vec::new();
        append_runtime_gateway_models(&mut models);
        let ids: Vec<&str> = models.iter().map(|model| model.id.as_str()).collect();

        for expected in [
            "auto-kiro",
            "deepseek-3.2",
            "glm-5",
            "minimax-m2.1",
            "minimax-m2.5",
            "qwen3-coder-next",
        ] {
            assert!(ids.contains(&expected), "{expected} missing");
        }
        assert!(!ids.contains(&"auto"));
    }

    #[tokio::test]
    async fn test_health_check_uses_group_level_credential_availability() {
        let mut free = KiroCredentials::default();
        free.groups = vec!["Free".to_string()];

        let mut disabled_power = KiroCredentials::default();
        disabled_power.groups = vec!["Power".to_string()];
        disabled_power.disabled = true;
        disabled_power.disable_reason = Some("Manual".to_string());

        let token_manager = Arc::new(
            MultiTokenManager::new(
                Config::default(),
                vec![free, disabled_power],
                None,
                None,
                false,
            )
            .unwrap(),
        );
        let provider = Arc::new(KiroProvider::new(token_manager));
        let state = AppState::new(
            "test-api-key",
            Arc::new(parking_lot::RwLock::new(
                crate::anthropic::middleware::PromptCacheRuntime::new(300, false, None),
            )),
        )
        .with_kiro_provider(provider);
        let auth = AuthIdentity {
            key_id: 1,
            key_source: TraceKeySource::MasterApiKey,
            group: Some("Power".to_string()),
        };
        let payload = MessagesRequest {
            model: "claude-sonnet-4".to_string(),
            max_tokens: 16,
            messages: vec![Message {
                role: "user".to_string(),
                content: serde_json::json!("ping"),
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

        let response = post_messages(
            OriginalUri("/v1/messages".parse().unwrap()),
            State(state),
            axum::Extension(auth),
            HeaderMap::new(),
            JsonExtractor(payload),
        )
        .await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    // 注意：is_no_credentials_error / is_quota_exhausted_error / is_all_credentials_cooling_down_error
    // 测试已迁移至 error_map::tests（TASK-001），这里不再重复。

    #[test]
    fn test_adaptive_shrink_removes_only_history_images() {
        let big = "A".repeat(20_000);
        let mut kiro_request = KiroRequest {
            conversation_state: ConversationState::new("conv-1")
                .with_current_message(CurrentMessage::new(
                    UserInputMessage::new("current", "model")
                        .with_images(vec![KiroImage::from_base64("png", big.clone())]),
                ))
                .with_history(vec![KiroMessage::user("history", "model")]),
            profile_arn: None,
            additional_model_request_fields: None,
        };
        if let KiroMessage::User(user) = &mut kiro_request.conversation_state.history[0] {
            user.user_input_message.images = vec![KiroImage::from_base64("png", big.clone())];
        }

        let removed = kiro_request.conversation_state.remove_history_images();

        assert_eq!(removed, 1);
        assert_eq!(
            kiro_request
                .conversation_state
                .current_message
                .user_input_message
                .images
                .len(),
            1
        );
        assert!(match &kiro_request.conversation_state.history[0] {
            KiroMessage::User(user) => user.user_input_message.images.is_empty(),
            _ => false,
        });
    }

    #[test]
    fn test_adaptive_shrink_triggers_on_token_limit_under_byte_limit() {
        let content = "A".repeat(20_000);
        let mut kiro_request = KiroRequest {
            conversation_state: ConversationState::new("conv-token").with_current_message(
                CurrentMessage::new(UserInputMessage::new(content, "claude-sonnet-4.5")),
            ),
            profile_arn: None,
            additional_model_request_fields: None,
        };
        let mut request_body = serde_json::to_string(&kiro_request).unwrap();
        let max_body = request_body.len() + 10_000;
        let max_tokens = 4_000;

        assert!(request_body.len() <= max_body);
        assert!(estimate_request_body_tokens(&request_body) > max_tokens);

        let outcome = adaptive_shrink_request_body(
            &mut kiro_request,
            &crate::model::config::CompressionConfig::default(),
            max_body,
            max_tokens,
            &mut request_body,
        )
        .expect("adaptive shrink should serialize")
        .expect("token limit should trigger adaptive shrink");

        assert!(outcome.iters > 0);
        assert!(outcome.final_bytes < outcome.initial_bytes);
        assert!(request_body.len() <= max_body);
        assert!(estimate_request_body_tokens(&request_body) <= max_tokens);
    }

    #[test]
    fn test_improperly_formed_request_message_mentions_common_causes() {
        let response = map_kiro_provider_error_to_response(
            "{}",
            anyhow::anyhow!("400 Improperly formed request"),
            None,
        );
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
