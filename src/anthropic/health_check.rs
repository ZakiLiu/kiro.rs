use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
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

/// 计算探测前缀（前缀匹配，大小写不敏感）
const CALC_PROBE_PREFIX: &str = "calculate and respond with only the number, nothing else.";

const MOCK_REPLIES: &[&str] = &[
    "Hi! How can I help you today?",
    "Hi! How can I help you?",
    "Hello! How can I help you today?",
    "Hello! How can I help you?",
];

pub const MOCK_INPUT_TOKENS: i32 = 14;
pub const MOCK_OUTPUT_TOKENS: i32 = 7;
pub const MOCK_CACHE_READ_TOKENS: i32 = 456;
pub const MOCK_CREDIT_USAGE: f64 = 0.0101;

/// 检测结果
pub enum HealthCheckKind {
    None,
    Greeting,
    /// 计算探测：携带计算结果
    CalcProbe(String),
}

pub fn detect_health_check(payload: &MessagesRequest) -> HealthCheckKind {
    if payload.messages.len() != 1 {
        return HealthCheckKind::None;
    }
    let msg = &payload.messages[0];
    if msg.role != "user" {
        return HealthCheckKind::None;
    }
    if payload.tools.as_ref().is_some_and(|t| !t.is_empty()) {
        return HealthCheckKind::None;
    }
    let text = match &msg.content {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Array(blocks) => {
            let texts: Vec<&str> = blocks
                .iter()
                .filter_map(|b| {
                    if b.get("type")?.as_str()? == "text" {
                        b.get("text")?.as_str()
                    } else {
                        None
                    }
                })
                .collect();
            if texts.len() != 1 {
                return HealthCheckKind::None;
            }
            texts[0].trim().to_string()
        }
        _ => return HealthCheckKind::None,
    };
    let lower = text.to_lowercase();
    // 短问候词（≤ 20 字符）
    if text.len() <= 20 && GREETING_PATTERNS.iter().any(|p| lower == *p) {
        return HealthCheckKind::Greeting;
    }
    // 计算探测："Calculate and respond with ONLY the number..." + 算式
    if lower.starts_with(CALC_PROBE_PREFIX) {
        let remainder = text[CALC_PROBE_PREFIX.len()..].trim();
        if let Some(answer) = eval_simple_math(remainder) {
            return HealthCheckKind::CalcProbe(answer);
        }
    }
    HealthCheckKind::None
}

fn pick_reply(kind: &HealthCheckKind) -> String {
    match kind {
        HealthCheckKind::CalcProbe(answer) => answer.clone(),
        _ => {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos() as usize;
            MOCK_REPLIES[nanos % MOCK_REPLIES.len()].to_string()
        }
    }
}

/// 简易数学表达式求值（支持 +、-、*、/、带括号）
fn eval_simple_math(expr: &str) -> Option<String> {
    // 提取最后一个 "Q:" 行（未回答的问题）
    let expr = expr
        .lines()
        .rev()
        .find(|l| l.trim_start().starts_with("Q:") || l.contains("= ?") || l.contains("=?"))
        .or_else(|| {
            expr.lines()
                .find(|l| l.contains('+') || l.contains('*') || l.contains('-') || l.contains('/'))
        })
        .unwrap_or(expr);
    let expr = expr.strip_prefix("Q:").unwrap_or(expr).trim();
    let expr = expr
        .strip_suffix("= ?")
        .or_else(|| expr.strip_suffix("=?"))
        .unwrap_or(expr)
        .trim();
    let expr = expr.strip_suffix('=').unwrap_or(expr).trim();

    if expr.is_empty() {
        return None;
    }

    // 只允许数字、运算符、空格、括号、小数点
    if !expr
        .chars()
        .all(|c| c.is_ascii_digit() || "+-*/().% ".contains(c))
    {
        return None;
    }

    let result = eval_expr(&mut expr.chars().peekable())?;
    // 整数结果不带小数点
    if (result - result.round()).abs() < 1e-9 {
        Some(format!("{}", result as i64))
    } else {
        Some(format!("{:.2}", result))
    }
}

fn eval_expr(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<f64> {
    let mut result = eval_term(chars)?;
    loop {
        skip_spaces(chars);
        match chars.peek() {
            Some('+') => {
                chars.next();
                result += eval_term(chars)?;
            }
            Some('-') => {
                chars.next();
                result -= eval_term(chars)?;
            }
            _ => break,
        }
    }
    Some(result)
}

fn eval_term(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<f64> {
    let mut result = eval_factor(chars)?;
    loop {
        skip_spaces(chars);
        match chars.peek() {
            Some('*') => {
                chars.next();
                result *= eval_factor(chars)?;
            }
            Some('/') => {
                chars.next();
                let d = eval_factor(chars)?;
                if d == 0.0 {
                    return None;
                }
                result /= d;
            }
            Some('%') => {
                chars.next();
                let d = eval_factor(chars)?;
                if d == 0.0 {
                    return None;
                }
                result %= d;
            }
            _ => break,
        }
    }
    Some(result)
}

fn eval_factor(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<f64> {
    skip_spaces(chars);
    if chars.peek() == Some(&'(') {
        chars.next();
        let result = eval_expr(chars)?;
        skip_spaces(chars);
        if chars.peek() == Some(&')') {
            chars.next();
        }
        return Some(result);
    }
    // 负号
    let neg = if chars.peek() == Some(&'-') {
        chars.next();
        true
    } else {
        false
    };
    skip_spaces(chars);
    let mut num = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            chars.next();
        } else {
            break;
        }
    }
    let val: f64 = num.parse().ok()?;
    Some(if neg { -val } else { val })
}

fn skip_spaces(chars: &mut std::iter::Peekable<std::str::Chars>) {
    while chars.peek() == Some(&' ') {
        chars.next();
    }
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

pub fn mock_non_stream_response(model: &str, kind: &HealthCheckKind) -> Response {
    let msg_id = format!("msg_{}", Uuid::new_v4().to_string().replace('-', ""));
    let body = json!({
        "id": msg_id,
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": pick_reply(kind)}],
        "model": model,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": mock_usage()
    });
    (StatusCode::OK, anthropic_response_headers(), Json(body)).into_response()
}

pub fn mock_stream_response(model: &str, kind: &HealthCheckKind) -> Response {
    let msg_id = format!("msg_{}", Uuid::new_v4().to_string().replace('-', ""));
    let reply = pick_reply(kind);

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

    (
        StatusCode::OK,
        headers,
        axum::body::Body::from_stream(stream),
    )
        .into_response()
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
