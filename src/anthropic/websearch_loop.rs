//! Agentic WebSearch 循环
//!
//! 混合工具场景：上游返回 tool_use(web_search) → 本地 MCP 搜索 → 结果注入 → 重新发送 → 循环。
//! 非 web_search 工具（exec 等）不进入循环，直接返回客户端。
//! 移植自 Kiro-RS-Tool websearch_loop.rs，适配当前项目架构。

use std::convert::Infallible;
use std::sync::Arc;

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures::{StreamExt, stream};
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

use crate::kiro::model::events::Event;
use crate::kiro::model::requests::kiro::KiroRequest;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::kiro::provider::KiroProvider;
use crate::model::config::CompressionConfig;
use crate::token;

use super::converter::convert_request;
use super::stream::{SseEvent, ToolJsonAccumulator, THINKING_SIGNATURE_PLACEHOLDER};
use super::types::{ErrorResponse, Message, MessagesRequest};
use super::websearch::{self, WebSearchResults};

const MAX_WEB_SEARCH_ROUNDS: usize = 5;

type SseBytes = Result<Bytes, Infallible>;

enum StreamStartup {
    Started,
    Failed(Response),
}

struct StreamFirstByteMarker {
    tx: mpsc::Sender<SseBytes>,
    startup_tx: Option<oneshot::Sender<StreamStartup>>,
    started: bool,
}

impl StreamFirstByteMarker {
    fn new(tx: mpsc::Sender<SseBytes>, startup_tx: oneshot::Sender<StreamStartup>) -> Self {
        Self {
            tx,
            startup_tx: Some(startup_tx),
            started: false,
        }
    }

    async fn mark_first_upstream_chunk(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let _ = self.tx.send(Ok(create_ping_sse())).await;
        if let Some(tx) = self.startup_tx.take() {
            let _ = tx.send(StreamStartup::Started);
        }
    }

    fn mark_started_before_final_flush(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        if let Some(tx) = self.startup_tx.take() {
            let _ = tx.send(StreamStartup::Started);
        }
    }

    fn fail_before_start(&mut self, response: Response) -> bool {
        if self.started {
            return false;
        }
        self.started = true;
        if let Some(tx) = self.startup_tx.take() {
            let _ = tx.send(StreamStartup::Failed(response));
        }
        true
    }
}

fn create_ping_sse() -> Bytes {
    Bytes::from("event: ping\ndata: {\"type\": \"ping\"}\n\n")
}

fn create_error_sse(error_type: &str, message: impl Into<String>) -> Bytes {
    Bytes::from(
        SseEvent::new(
            "error",
            json!({
                "type": "error",
                "error": {
                    "type": error_type,
                    "message": message.into(),
                }
            }),
        )
        .to_sse_string(),
    )
}

struct RoundOutcome {
    text: String,
    reasoning: String,
    redacted_reasoning: String,
    tool_uses: Vec<DecodedToolUse>,
    context_input_tokens: Option<i32>,
    credits: f64,
    stop_reason_override: Option<String>,
    stream_error: bool,
    tool_json_error: Option<String>,
}

struct DecodedToolUse {
    id: String,
    name: String,
    input: Value,
}

impl DecodedToolUse {
    fn query(&self) -> String {
        self.input
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }
}

fn should_search_round(round_idx: usize, tool_uses: &[DecodedToolUse]) -> bool {
    let only_web_search = !tool_uses.is_empty() && tool_uses.iter().all(|t| t.name == "web_search");
    only_web_search && round_idx < MAX_WEB_SEARCH_ROUNDS
}

async fn decode_round(
    response: reqwest::Response,
    model: &str,
    tool_name_map: &std::collections::HashMap<String, String>,
    mut first_byte_marker: Option<&mut StreamFirstByteMarker>,
) -> RoundOutcome {
    let mut body_stream = response.bytes_stream();
    let mut decoder = EventStreamDecoder::new();

    let mut text = String::new();
    let mut reasoning = String::new();
    let mut redacted_reasoning = String::new();
    let mut tool_accumulator = ToolJsonAccumulator::new();
    let mut tool_uses: Vec<DecodedToolUse> = Vec::new();
    let mut context_input_tokens: Option<i32> = None;
    let mut credits = 0.0;
    let mut stop_reason_override: Option<String> = None;
    let mut stream_error = false;
    let mut tool_json_error: Option<String> = None;

    while let Some(chunk) = body_stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("web_search loop: 上游响应流读取失败: {}", e);
                stream_error = true;
                break;
            }
        };
        if let Some(marker) = first_byte_marker.as_deref_mut() {
            marker.mark_first_upstream_chunk().await;
        }
        if let Err(e) = decoder.feed(&chunk) {
            tracing::warn!("buffer overflow: {}", e);
        }
        for result in decoder.decode_iter() {
            let frame = match result {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("event 解码失败: {}", e);
                    continue;
                }
            };
            let event = match Event::from_frame(frame) {
                Ok(ev) => ev,
                Err(_) => continue,
            };
            match event {
                Event::AssistantResponse(resp) => text.push_str(&resp.content),
                Event::ReasoningContent(rc) => {
                    if !rc.text.is_empty() {
                        reasoning.push_str(&rc.text);
                    } else if let Some(redacted) = rc.redacted_content.as_ref() {
                        redacted_reasoning.push_str(redacted);
                    }
                }
                Event::ToolUse(tu) => {
                    match tool_accumulator.push(&tu, tool_name_map) {
                        Ok(Some(completed)) => {
                            tool_uses.push(DecodedToolUse {
                                id: completed.id,
                                name: completed.name,
                                input: completed.input,
                            });
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tracing::error!("{}", e);
                            tool_json_error = Some(e.message());
                        }
                    }
                }
                Event::ContextUsage(cu) => {
                    let window =
                        super::types::get_context_window_size(model);
                    let actual = (cu.context_usage_percentage * (window as f64) / 100.0) as i32;
                    context_input_tokens = Some(actual);
                    if cu.context_usage_percentage >= 100.0 {
                        stop_reason_override = Some("model_context_window_exceeded".to_string());
                    }
                }
                Event::Metering(m) => credits += m.usage,
                Event::Exception { exception_type, .. }
                    if exception_type == "ContentLengthExceededException" =>
                {
                    stop_reason_override = Some("max_tokens".to_string());
                }
                _ => {}
            }
        }
    }

    // 检测未完成的工具 JSON
    if tool_json_error.is_none()
        && let Err(e) = tool_accumulator.finish()
    {
        tracing::error!("{}", e);
        tool_json_error = Some(e.message());
    }

    // XML 泄漏过滤
    text = crate::kiro::model::events::strip_tool_use_xml_leaks(&text);

    RoundOutcome {
        text,
        reasoning,
        redacted_reasoning,
        tool_uses,
        context_input_tokens,
        credits,
        stop_reason_override,
        stream_error,
        tool_json_error,
    }
}

fn map_provider_error_to_response(err: anyhow::Error) -> Response {
    let ctx = super::error_map::ErrorRequestContext {
        was_compressed: false,
        request_body_bytes: 0,
        compression_iterations: None,
    };
    let category = super::error_map::classify(&err, &ctx);
    super::error_map::to_anthropic_response(&category, &err, &ctx)
}

async fn run_round(
    provider: &Arc<KiroProvider>,
    payload: &MessagesRequest,
    compression_config: &CompressionConfig,
    _fallback_input_tokens: i32,
    first_byte_marker: Option<&mut StreamFirstByteMarker>,
    group: Option<&str>,
) -> Result<(RoundOutcome, u64), Response> {
    let conversion = match convert_request(payload, compression_config, None) {
        Ok(c) => c,
        Err(e) => {
            let msg = e.to_string();
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("invalid_request_error", msg)),
            )
                .into_response());
        }
    };

    let kiro_request = KiroRequest {
        conversation_state: conversion.conversation_state,
        profile_arn: None,
        additional_model_request_fields: None,
    };
    let request_body = match serde_json::to_string(&kiro_request) {
        Ok(b) => b,
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "internal_error",
                    format!("failed to serialize request: {}", e),
                )),
            )
                .into_response());
        }
    };

    let call_result = match provider
        .call_api_stream(&request_body, None, group)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err(map_provider_error_to_response(e));
        }
    };
    let credential_id = call_result.credential_id;
    let outcome = decode_round(
        call_result.response,
        &payload.model,
        &conversion.tool_name_map,
        first_byte_marker,
    )
    .await;
    if let Some(message) = &outcome.tool_json_error {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse::new(
                "upstream_tool_json_error",
                message.clone(),
            )),
        )
            .into_response());
    }
    if outcome.stream_error {
        return Err((
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse::new(
                "upstream_error",
                "Upstream response stream ended unexpectedly during the web_search loop.",
            )),
        )
            .into_response());
    }
    Ok((outcome, credential_id))
}

fn append_search_round(
    payload: &mut MessagesRequest,
    round: &RoundOutcome,
    searched: &[Option<WebSearchResults>],
    presentation: &mut Vec<Value>,
    thinking_enabled: bool,
) {
    let mut assistant_content: Vec<Value> = Vec::new();
    if thinking_enabled && !round.reasoning.is_empty() {
        assistant_content.push(json!({
            "type": "thinking",
            "thinking": round.reasoning,
            "signature": THINKING_SIGNATURE_PLACEHOLDER
        }));
    }
    if thinking_enabled && !round.redacted_reasoning.is_empty() {
        assistant_content.push(json!({
            "type": "redacted_thinking",
            "data": round.redacted_reasoning
        }));
    }
    if !round.text.is_empty() {
        assistant_content.push(json!({"type": "text", "text": round.text}));
    }
    for tu in &round.tool_uses {
        assistant_content.push(json!({
            "type": "tool_use", "id": tu.id, "name": tu.name, "input": tu.input
        }));
    }
    payload.messages.push(Message {
        role: "assistant".to_string(),
        content: Value::Array(assistant_content),
    });

    let mut user_content: Vec<Value> = Vec::new();
    for (tu, results) in round.tool_uses.iter().zip(searched.iter()) {
        let query = tu.query();
        let summary = websearch::generate_search_summary(&query, results);
        user_content.push(json!({
            "type": "tool_result", "tool_use_id": tu.id, "content": summary
        }));

        let (srv_id, _mcp) = websearch::create_mcp_request(&query);
        presentation.push(json!({
            "type": "server_tool_use", "id": srv_id, "name": "web_search",
            "input": {"query": query}
        }));
        presentation.push(json!({
            "type": "web_search_tool_result",
            "content": build_result_block(results)
        }));
    }
    payload.messages.push(Message {
        role: "user".to_string(),
        content: Value::Array(user_content),
    });
}

fn build_result_block(results: &Option<WebSearchResults>) -> Vec<Value> {
    match results {
        Some(r) => r
            .results
            .iter()
            .map(|item| {
                let page_age = item.published_date.and_then(|ms| {
                    chrono::DateTime::from_timestamp_millis(ms)
                        .map(|dt| dt.format("%B %-d, %Y").to_string())
                });
                json!({
                    "type": "web_search_result",
                    "title": item.title,
                    "url": item.url,
                    "encrypted_content": item.snippet.clone().unwrap_or_default(),
                    "page_age": page_age
                })
            })
            .collect(),
        None => vec![],
    }
}

struct WebSearchLoopSuccess {
    model: String,
    content: Vec<Value>,
    stop_reason: String,
    input_tokens: i32,
    output_tokens: i32,
}

async fn run_web_search_loop_inner(
    provider: Arc<KiroProvider>,
    mut payload: MessagesRequest,
    compression_config: CompressionConfig,
    mut first_byte_marker: Option<&mut StreamFirstByteMarker>,
    group: Option<&str>,
) -> Result<WebSearchLoopSuccess, Response> {
    let fallback_input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;

    let mut presentation: Vec<Value> = Vec::new();
    let mut last_context_input: Option<i32> = None;
    let mut _total_credits = 0.0;
    let thinking_enabled = payload.thinking.as_ref().is_some_and(|t| t.is_enabled());

    for round_idx in 0..=MAX_WEB_SEARCH_ROUNDS {
        let (round, _credential_id) = match run_round(
            &provider,
            &payload,
            &compression_config,
            fallback_input_tokens,
            first_byte_marker.as_deref_mut(),
            group,
        )
        .await
        {
            Ok(v) => v,
            Err(resp) => return Err(resp),
        };
        last_context_input = round.context_input_tokens.or(last_context_input);
        _total_credits += round.credits;

        if should_search_round(round_idx, &round.tool_uses) {
            let mut searched: Vec<Option<WebSearchResults>> =
                Vec::with_capacity(round.tool_uses.len());
            for tu in &round.tool_uses {
                let (_id, mcp_request) = websearch::create_mcp_request(&tu.query());
                match websearch::call_mcp_api(&provider, &mcp_request, group).await {
                    Ok(resp) => searched.push(websearch::parse_search_results(&resp.response)),
                    Err(e) => {
                        tracing::warn!("web_search MCP 调用失败: {}", e);
                        return Err(map_provider_error_to_response(e));
                    }
                }
            }
            append_search_round(
                &mut payload,
                &round,
                &searched,
                &mut presentation,
                thinking_enabled,
            );
            continue;
        }

        let stop_reason = round.stop_reason_override.clone().unwrap_or_else(|| {
            if round.tool_uses.is_empty() {
                "end_turn".to_string()
            } else {
                "tool_use".to_string()
            }
        });
        let total_input = last_context_input.unwrap_or(fallback_input_tokens);

        let mut content: Vec<Value> = presentation.clone();
        if thinking_enabled && !round.reasoning.is_empty() {
            content.push(json!({
                "type": "thinking",
                "thinking": round.reasoning,
                "signature": THINKING_SIGNATURE_PLACEHOLDER
            }));
        }
        if thinking_enabled && !round.redacted_reasoning.is_empty() {
            content.push(json!({
                "type": "redacted_thinking",
                "data": round.redacted_reasoning
            }));
        }
        if !round.text.is_empty() {
            content.push(json!({"type": "text", "text": round.text}));
        }
        for tu in &round.tool_uses {
            content.push(json!({
                "type": "tool_use", "id": tu.id, "name": tu.name, "input": tu.input
            }));
        }

        let output_tokens = token::estimate_output_tokens(&content);

        return Ok(WebSearchLoopSuccess {
            model: payload.model,
            content,
            stop_reason,
            input_tokens: total_input,
            output_tokens,
        });
    }

    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse::new(
            "internal_error",
            "web_search loop exited unexpectedly",
        )),
    )
        .into_response())
}

pub(super) async fn run_web_search_loop(
    provider: Arc<KiroProvider>,
    payload: MessagesRequest,
    stream_client: bool,
    compression_config: CompressionConfig,
    group: Option<&str>,
) -> Response {
    if stream_client {
        return render_deferred_sse(provider, payload, compression_config, group).await;
    }

    match run_web_search_loop_inner(provider, payload, compression_config, None, group).await {
        Ok(success) => render_json(
            &success.model,
            success.content,
            &success.stop_reason,
            success.input_tokens,
            success.output_tokens,
        ),
        Err(resp) => resp,
    }
}

async fn render_deferred_sse(
    provider: Arc<KiroProvider>,
    payload: MessagesRequest,
    compression_config: CompressionConfig,
    group: Option<&str>,
) -> Response {
    let (tx, rx) = mpsc::channel::<SseBytes>(32);
    let (startup_tx, startup_rx) = oneshot::channel::<StreamStartup>();
    let group_owned = group.map(|s| s.to_string());

    tokio::spawn(async move {
        let mut marker = StreamFirstByteMarker::new(tx.clone(), startup_tx);
        let result = run_web_search_loop_inner(
            provider,
            payload,
            compression_config,
            Some(&mut marker),
            group_owned.as_deref(),
        )
        .await;

        match result {
            Ok(success) => {
                marker.mark_started_before_final_flush();
                for event in build_sse_events(
                    &success.model,
                    success.content,
                    &success.stop_reason,
                    success.input_tokens,
                    success.output_tokens,
                ) {
                    if tx
                        .send(Ok(Bytes::from(event.to_sse_string())))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            Err(resp) => {
                if !marker.fail_before_start(resp) {
                    let _ = tx
                        .send(Ok(create_error_sse(
                            "api_error",
                            "web_search loop failed after upstream stream had started",
                        )))
                        .await;
                }
            }
        }
    });

    match startup_rx.await {
        Ok(StreamStartup::Started) => {
            let stream = stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|item| (item, rx))
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/event-stream")
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::CONNECTION, "keep-alive")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        Ok(StreamStartup::Failed(resp)) => resp,
        Err(_) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse::new(
                "upstream_error",
                "web_search loop ended before the response stream could start",
            )),
        )
            .into_response(),
    }
}

fn render_json(
    model: &str,
    content: Vec<Value>,
    stop_reason: &str,
    input_tokens: i32,
    output_tokens: i32,
) -> Response {
    let body = json!({
        "id": format!("msg_{}", Uuid::new_v4().to_string().replace('-', "")),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
        }
    });
    (StatusCode::OK, Json(body)).into_response()
}

fn build_sse_events(
    model: &str,
    content: Vec<Value>,
    stop_reason: &str,
    input_tokens: i32,
    output_tokens: i32,
) -> Vec<SseEvent> {
    let mut events = Vec::new();
    let message_id = format!("msg_{}", &Uuid::new_v4().to_string().replace('-', "")[..24]);

    events.push(SseEvent::new(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": input_tokens,
                    "output_tokens": 0,
                }
            }
        }),
    ));

    for (index, block) in content.iter().enumerate() {
        let index = index as i32;
        let btype = block.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match btype {
            "thinking" => {
                let thinking = block.get("thinking").and_then(|v| v.as_str()).unwrap_or("");
                events.push(SseEvent::new(
                    "content_block_start",
                    json!({
                        "type": "content_block_start", "index": index,
                        "content_block": {"type": "thinking", "thinking": ""}
                    }),
                ));
                if !thinking.is_empty() {
                    events.push(SseEvent::new(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta", "index": index,
                            "delta": {"type": "thinking_delta", "thinking": thinking}
                        }),
                    ));
                }
                events.push(SseEvent::new(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta", "index": index,
                        "delta": {"type": "signature_delta", "signature": THINKING_SIGNATURE_PLACEHOLDER}
                    }),
                ));
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                ));
            }
            "redacted_thinking" => {
                let data = block.get("data").and_then(|v| v.as_str()).unwrap_or("");
                events.push(SseEvent::new(
                    "content_block_start",
                    json!({
                        "type": "content_block_start", "index": index,
                        "content_block": {"type": "redacted_thinking", "data": data}
                    }),
                ));
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                ));
            }
            "text" => {
                let text = block.get("text").and_then(|v| v.as_str()).unwrap_or("");
                events.push(SseEvent::new(
                    "content_block_start",
                    json!({
                        "type": "content_block_start", "index": index,
                        "content_block": {"type": "text", "text": ""}
                    }),
                ));
                events.push(SseEvent::new(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta", "index": index,
                        "delta": {"type": "text_delta", "text": text}
                    }),
                ));
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                ));
            }
            "tool_use" => {
                let id = block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let name = block.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                let partial = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                events.push(SseEvent::new(
                    "content_block_start",
                    json!({
                        "type": "content_block_start", "index": index,
                        "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}
                    }),
                ));
                events.push(SseEvent::new(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta", "index": index,
                        "delta": {"type": "input_json_delta", "partial_json": partial}
                    }),
                ));
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                ));
            }
            "server_tool_use" | "web_search_tool_result" => {
                events.push(SseEvent::new(
                    "content_block_start",
                    json!({
                        "type": "content_block_start", "index": index,
                        "content_block": block
                    }),
                ));
                events.push(SseEvent::new(
                    "content_block_stop",
                    json!({"type": "content_block_stop", "index": index}),
                ));
            }
            _ => {}
        }
    }

    events.push(SseEvent::new(
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": stop_reason},
            "usage": {"output_tokens": output_tokens}
        }),
    ));
    events.push(SseEvent::new(
        "message_stop",
        json!({"type": "message_stop"}),
    ));

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::websearch::{WebSearchResult, WebSearchResults};

    fn tu(name: &str) -> DecodedToolUse {
        DecodedToolUse {
            id: format!("toolu_{}", name),
            name: name.to_string(),
            input: json!({"query": "rust 2026"}),
        }
    }

    #[test]
    fn round_with_only_web_search_continues() {
        let tools = vec![tu("web_search"), tu("web_search")];
        assert!(should_search_round(0, &tools));
        assert!(should_search_round(MAX_WEB_SEARCH_ROUNDS - 1, &tools));
    }

    #[test]
    fn round_with_exec_does_not_enter_loop() {
        let mixed = vec![tu("web_search"), tu("exec")];
        assert!(!should_search_round(0, &mixed));
        let exec_only = vec![tu("exec")];
        assert!(!should_search_round(0, &exec_only));
    }

    #[test]
    fn round_with_no_tool_use_does_not_enter_loop() {
        let empty: Vec<DecodedToolUse> = vec![];
        assert!(!should_search_round(0, &empty));
    }

    #[test]
    fn round_at_limit_stops_even_if_web_search() {
        let tools = vec![tu("web_search")];
        assert!(!should_search_round(MAX_WEB_SEARCH_ROUNDS, &tools));
        assert!(!should_search_round(MAX_WEB_SEARCH_ROUNDS + 1, &tools));
    }

    #[test]
    fn result_block_maps_contract_a_fields() {
        let results = WebSearchResults {
            results: vec![WebSearchResult {
                title: "Rust 1.99".to_string(),
                url: "https://example.com/rust".to_string(),
                snippet: Some("Rust 1.99 released".to_string()),
                published_date: None,
                id: None,
                domain: None,
                max_verbatim_word_limit: None,
                public_domain: None,
            }],
            total_results: Some(1),
            query: Some("rust".to_string()),
            error: None,
        };
        let block = build_result_block(&Some(results));
        assert_eq!(block.len(), 1);
        assert_eq!(block[0]["type"], "web_search_result");
        assert_eq!(block[0]["title"], "Rust 1.99");
        assert_eq!(block[0]["url"], "https://example.com/rust");
        assert_eq!(block[0]["encrypted_content"], "Rust 1.99 released");
    }

    #[test]
    fn result_block_none_is_empty() {
        assert!(build_result_block(&None).is_empty());
    }

    #[test]
    fn sse_events_render_search_presentation_and_keep_exec() {
        let content = vec![
            json!({"type": "server_tool_use", "id": "srvtoolu_x", "name": "web_search", "input": {"query": "q"}}),
            json!({"type": "web_search_tool_result", "content": []}),
            json!({"type": "text", "text": "done"}),
            json!({"type": "tool_use", "id": "toolu_exec", "name": "exec", "input": {"cmd": "ls"}}),
        ];
        let events = build_sse_events("claude-opus-4-8", content, "tool_use", 10, 5);

        assert_eq!(events.first().unwrap().event, "message_start");
        assert_eq!(events.last().unwrap().event, "message_stop");
        let delta = events.iter().find(|e| e.event == "message_delta").unwrap();
        assert_eq!(delta.data["delta"]["stop_reason"], "tool_use");

        let has_server_tool = events.iter().any(|e| {
            e.event == "content_block_start" && e.data["content_block"]["type"] == "server_tool_use"
        });
        assert!(has_server_tool);

        let has_result = events.iter().any(|e| {
            e.event == "content_block_start"
                && e.data["content_block"]["type"] == "web_search_tool_result"
        });
        assert!(has_result);

        let has_exec = events.iter().any(|e| {
            e.event == "content_block_start"
                && e.data["content_block"]["type"] == "tool_use"
                && e.data["content_block"]["name"] == "exec"
        });
        assert!(has_exec);
    }

    #[test]
    fn sse_events_render_redacted_thinking_without_plaintext_delta() {
        let content = vec![json!({
            "type": "redacted_thinking",
            "data": "encrypted-thinking"
        })];
        let events = build_sse_events("claude-opus-4-8", content, "end_turn", 10, 5);

        let start = events
            .iter()
            .find(|e| {
                e.event == "content_block_start"
                    && e.data["content_block"]["type"] == "redacted_thinking"
            })
            .expect("redacted thinking block should be rendered");
        assert_eq!(
            start.data["content_block"]["data"].as_str(),
            Some("encrypted-thinking")
        );
        assert!(events.iter().all(|e| {
            !(e.event == "content_block_delta" && e.data["delta"]["type"] == "thinking_delta")
        }));
    }
}
