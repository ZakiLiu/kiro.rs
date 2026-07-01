use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use serde_json::json;
use std::convert::Infallible;
use uuid::Uuid;

use super::handlers::anthropic_response_headers;
use super::types::MessagesRequest;

const GREETING_PATTERNS: &[&str] = &[
    "hi",
    "hello",
    "hey",
    "ping",
    "test",
    "say hi",
    "say hello",
    "你好",
    "嗨",
    "测试",
];

const MOCK_REPLIES: &[&str] = &[
    "Hi! How can I help you today?",
    "Hi! How can I help you?",
    "Hello! How can I help you today?",
    "Hello! How can I help you?",
];

const MOCK_INPUT_TOKENS: i32 = 14;
const MOCK_OUTPUT_TOKENS: i32 = 7;
const MOCK_CACHE_READ_TOKENS: i32 = 456;
const MOCK_CREDIT_USAGE: f64 = 0.0101;

pub fn is_health_check_request(payload: &MessagesRequest) -> bool {
    if payload.messages.len() != 1 {
        return false;
    }
    let msg = &payload.messages[0];
    if msg.role != "user" {
        return false;
    }
    if payload.tools.as_ref().is_some_and(|t| !t.is_empty()) {
        return false;
    }
    let text = match &msg.content {
        serde_json::Value::String(s) => s.trim().to_string(),
        _ => return false,
    };
    if text.len() > 20 {
        return false;
    }
    let lower = text.to_lowercase();
    GREETING_PATTERNS.iter().any(|p| lower == *p)
}

fn pick_reply() -> &'static str {
    let idx = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as usize
        % MOCK_REPLIES.len();
    MOCK_REPLIES[idx]
}

fn mock_usage() -> serde_json::Value {
    json!({
        "input_tokens": MOCK_INPUT_TOKENS,
        "output_tokens": MOCK_OUTPUT_TOKENS,
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": MOCK_CACHE_READ_TOKENS,
        "credit_usage": MOCK_CREDIT_USAGE,
        "credit_unit": "credit",
        "credit_unit_plural": "credits",
        "cache_creation": {
            "ephemeral_5m_input_tokens": 0,
            "ephemeral_1h_input_tokens": 0
        }
    })
}

pub fn mock_non_stream_response(model: &str) -> Response {
    let msg_id = format!("msg_{}", Uuid::new_v4().to_string().replace('-', ""));
    let body = json!({
        "id": msg_id,
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": pick_reply()}],
        "model": model,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": mock_usage()
    });
    (StatusCode::OK, anthropic_response_headers(), Json(body)).into_response()
}

pub fn mock_stream_response(model: &str) -> Response {
    let msg_id = format!("msg_{}", Uuid::new_v4().to_string().replace('-', ""));
    let reply = pick_reply();

    let message_start = format!(
        "event: message_start\ndata: {}\n\n",
        json!({
            "type": "message_start",
            "message": {
                "id": msg_id,
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": model,
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {
                    "input_tokens": MOCK_INPUT_TOKENS,
                    "output_tokens": 1,
                    "cache_creation_input_tokens": 0,
                    "cache_read_input_tokens": MOCK_CACHE_READ_TOKENS
                }
            }
        })
    );

    let content_start = format!(
        "event: content_block_start\ndata: {}\n\n",
        json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}})
    );

    let content_delta = format!(
        "event: content_block_delta\ndata: {}\n\n",
        json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": reply}})
    );

    let content_stop = format!(
        "event: content_block_stop\ndata: {}\n\n",
        json!({"type": "content_block_stop", "index": 0})
    );

    let message_delta = format!(
        "event: message_delta\ndata: {}\n\n",
        json!({
            "type": "message_delta",
            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
            "usage": mock_usage()
        })
    );

    let message_stop = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

    let body = format!(
        "{}{}{}{}{}{}",
        message_start, content_start, content_delta, content_stop, message_delta, message_stop
    );

    let stream = futures::stream::once(async move { Ok::<Bytes, Infallible>(Bytes::from(body)) });

    let mut headers = anthropic_response_headers();
    headers.insert("content-type", "text/event-stream".parse().unwrap());
    headers.insert("cache-control", "no-cache".parse().unwrap());

    (StatusCode::OK, headers, axum::body::Body::from_stream(stream)).into_response()
}

pub fn mock_unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        anthropic_response_headers(),
        Json(json!({
            "type": "error",
            "error": {
                "type": "authentication_error",
                "message": "No available credentials for this group"
            }
        })),
    )
        .into_response()
}
