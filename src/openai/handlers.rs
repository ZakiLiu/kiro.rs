//! OpenAI 兼容 API Handler

use std::convert::Infallible;

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::json;
use tokio::time::{Instant, interval_at};
use std::time::Duration;

use crate::admin::trace_db::TraceAttempt;
use crate::anthropic::handlers::record_request_telemetry;
use crate::anthropic::middleware::{AppState, AuthIdentity};
use crate::kiro::model::events::Event;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::token;

use super::converter::convert_openai_to_kiro;
use super::stream::OpenAIStreamContext;
use super::types::*;

pub async fn post_chat_completions(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthIdentity>,
    Json(payload): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    tracing::info!(model = %payload.model, stream = %payload.stream, "OpenAI /v1/chat/completions");

    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!(ErrorResponse::new("server_error", "服务未就绪"))),
            )
                .into_response();
        }
    };

    let is_stream = payload.stream;

    let conversion = match convert_openai_to_kiro(&payload, state.profile_arn.clone()) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!(ErrorResponse::new("invalid_request_error", e))),
            )
                .into_response();
        }
    };

    let request_body = match serde_json::to_string(&conversion.kiro_request) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!(ErrorResponse::new("server_error", format!("序列化失败: {}", e)))),
            )
                .into_response();
        }
    };

    let input_tokens = token::count_tokens(&request_body) as i32;

    if is_stream {
        handle_stream(provider, &request_body, &conversion.model, input_tokens, &state, &auth).await
    } else {
        handle_non_stream(provider, &request_body, &conversion.model, input_tokens, &state, &auth).await
    }
}

async fn handle_stream(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    state: &AppState,
    auth: &AuthIdentity,
) -> Response {
    let start = Instant::now();
    let result = match provider.call_api_stream(request_body, None).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("upstream stream error: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!(ErrorResponse::new("api_error", "upstream service error"))),
            )
                .into_response();
        }
    };

    let credential_id = result.credential_id;
    let attempts = result.attempts;
    let response = result.response;
    let stream = create_openai_sse_stream(
        response, model.to_string(), input_tokens,
        state.clone(), auth.clone(), credential_id, attempts, start,
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "stream error",
            )
                .into_response()
        })
}

fn create_openai_sse_stream(
    response: reqwest::Response,
    model: String,
    input_tokens: i32,
    state: AppState,
    auth: AuthIdentity,
    credential_id: u64,
    attempts: Vec<TraceAttempt>,
    start: Instant,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let model = model;

    async_stream::stream! {
        let mut ctx = OpenAIStreamContext::new(&model, input_tokens);

        let initial = ctx.generate_initial_chunk();
        yield Ok(Bytes::from(initial));

        let mut decoder = EventStreamDecoder::new();
        let mut byte_stream = response.bytes_stream();
        let mut ping_interval = interval_at(
            Instant::now() + Duration::from_secs(25),
            Duration::from_secs(25),
        );

        loop {
            tokio::select! {
                chunk = byte_stream.next() => {
                    match chunk {
                        Some(Ok(data)) => {
                            if let Err(e) = decoder.feed(&data) {
                                tracing::warn!("decoder feed error: {}", e);
                                continue;
                            }
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => {
                                        match Event::from_frame(frame) {
                                            Ok(event) => {
                                                for sse_chunk in ctx.process_kiro_event(&event) {
                                                    yield Ok(Bytes::from(sse_chunk));
                                                }
                                            }
                                            Err(e) => {
                                                tracing::warn!("event parse error: {}", e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("decode error: {}", e);
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            tracing::error!("流读取错误: {}", e);
                            break;
                        }
                        None => break,
                    }
                }
                _ = ping_interval.tick() => {
                    yield Ok(Bytes::from(": ping\n\n"));
                }
            }
        }

        for chunk in ctx.generate_final_chunk() {
            yield Ok(Bytes::from(chunk));
        }

        let (final_input, final_output, final_cache_read) = ctx.usage_values();
        let duration_ms = start.elapsed().as_millis() as u64;
        record_request_telemetry(
            &state, &auth, &model, true, credential_id,
            final_input, final_output, 0, final_cache_read,
            0.0, duration_ms, "success", attempts,
        );
    }
}

async fn handle_non_stream(
    provider: std::sync::Arc<crate::kiro::provider::KiroProvider>,
    request_body: &str,
    model: &str,
    input_tokens: i32,
    state: &AppState,
    auth: &AuthIdentity,
) -> Response {
    let start = Instant::now();
    let result = match provider.call_api(request_body, None).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("upstream error: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!(ErrorResponse::new("api_error", "upstream service error"))),
            )
                .into_response();
        }
    };

    let credential_id = result.credential_id;
    let attempts = result.attempts;
    let response_bytes = match result.response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!(ErrorResponse::new("api_error", format!("读取响应失败: {}", e)))),
            )
                .into_response();
        }
    };

    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&response_bytes) {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!(ErrorResponse::new("api_error", format!("decoder feed error: {}", e)))),
        )
            .into_response();
    }

    let mut text_content = String::new();
    let mut reasoning_content = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut current_tool_input = String::new();
    let mut current_tool_name = String::new();
    let mut current_tool_id = String::new();
    let mut output_tokens = 0i32;
    let mut final_input_tokens = input_tokens;
    let mut cache_read_tokens = 0i32;
    let mut thinking_tokens = 0i32;

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => {
                match Event::from_frame(frame) {
                    Ok(Event::AssistantResponse(ev)) => {
                        if !ev.content.is_empty() {
                            text_content.push_str(&ev.content);
                            output_tokens += token::count_tokens(&ev.content) as i32;
                        }
                    }
                    Ok(Event::ReasoningContent(ev)) => {
                        if !ev.text.is_empty() {
                            reasoning_content.push_str(&ev.text);
                            thinking_tokens += token::count_tokens(&ev.text) as i32;
                        }
                    }
                    Ok(Event::ToolUse(ev)) => {
                        if ev.stop {
                            if !current_tool_id.is_empty() {
                                tool_calls.push(ToolCall {
                                    id: current_tool_id.clone(),
                                    call_type: "function".to_string(),
                                    function: ToolCallFunction {
                                        name: current_tool_name.clone(),
                                        arguments: current_tool_input.clone(),
                                    },
                                });
                                current_tool_input.clear();
                                current_tool_name.clear();
                                current_tool_id.clear();
                            }
                        } else if !ev.name.is_empty() && current_tool_name != ev.name {
                            if !current_tool_id.is_empty() {
                                tool_calls.push(ToolCall {
                                    id: current_tool_id.clone(),
                                    call_type: "function".to_string(),
                                    function: ToolCallFunction {
                                        name: current_tool_name.clone(),
                                        arguments: current_tool_input.clone(),
                                    },
                                });
                                current_tool_input.clear();
                            }
                            current_tool_name = ev.name.clone();
                            current_tool_id = ev.tool_use_id.clone();
                            if !ev.input.is_empty() {
                                current_tool_input.push_str(&ev.input);
                            }
                        } else if !ev.input.is_empty() {
                            current_tool_input.push_str(&ev.input);
                        }
                    }
                    Ok(Event::TokenUsage(ev)) => {
                        final_input_tokens = ev.uncached_input_tokens as i32;
                        output_tokens = ev.output_tokens as i32;
                        if let Some(cr) = ev.cache_read_input_tokens {
                            cache_read_tokens = cr as i32;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!("event parse error: {}", e);
                    }
                }
            }
            Err(e) => {
                tracing::warn!("decode error: {}", e);
            }
        }
    }

    if !current_tool_id.is_empty() {
        tool_calls.push(ToolCall {
            id: current_tool_id,
            call_type: "function".to_string(),
            function: ToolCallFunction {
                name: current_tool_name,
                arguments: current_tool_input,
            },
        });
    }

    let finish_reason = if !tool_calls.is_empty() { "tool_calls" } else { "stop" };

    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4().to_string().replace('-', "")),
        object: "chat.completion",
        created: chrono::Utc::now().timestamp(),
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChoiceMessage {
                role: "assistant",
                content: if !text_content.is_empty() {
                    Some(text_content)
                } else if tool_calls.is_empty() {
                    Some(String::new())
                } else {
                    None
                },
                reasoning_content: if reasoning_content.is_empty() { None } else { Some(reasoning_content) },
                tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
            },
            finish_reason: Some(finish_reason.to_string()),
        }],
        usage: Usage {
            prompt_tokens: final_input_tokens,
            completion_tokens: output_tokens,
            total_tokens: final_input_tokens + output_tokens,
            prompt_tokens_details: if cache_read_tokens > 0 {
                Some(PromptTokensDetails { cached_tokens: cache_read_tokens })
            } else {
                None
            },
            completion_tokens_details: if thinking_tokens > 0 {
                Some(CompletionTokensDetails { reasoning_tokens: thinking_tokens })
            } else {
                None
            },
        },
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    record_request_telemetry(
        state, auth, model, false, credential_id,
        final_input_tokens, output_tokens, 0, cache_read_tokens,
        0.0, duration_ms, "success", attempts,
    );

    (StatusCode::OK, Json(json!(response))).into_response()
}

pub async fn post_responses(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthIdentity>,
    Json(payload): Json<ResponsesRequest>,
) -> impl IntoResponse {
    tracing::info!(model = %payload.model, stream = %payload.stream, "OpenAI /v1/responses");

    let chat_req = match responses_to_chat_completion(&payload) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!(ErrorResponse::new("invalid_request_error", e))),
            )
                .into_response();
        }
    };

    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!(ErrorResponse::new("server_error", "服务未就绪"))),
            )
                .into_response();
        }
    };

    let conversion = match convert_openai_to_kiro(&chat_req, state.profile_arn.clone()) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!(ErrorResponse::new("invalid_request_error", e))),
            )
                .into_response();
        }
    };

    let request_body = match serde_json::to_string(&conversion.kiro_request) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!(ErrorResponse::new("server_error", format!("序列化失败: {}", e)))),
            )
                .into_response();
        }
    };

    let input_tokens = token::count_tokens(&request_body) as i32;

    // Responses API 强制走非流式路径再包装为 Responses 格式
    // 原因：Responses SSE 格式 (response.output_text.delta) 与 Chat Completion SSE 格式完全不同，
    // 直接复用 handle_stream 会返回错误的 SSE 事件类型
    let resp = handle_non_stream(provider, &request_body, &conversion.model, input_tokens, &state, &auth).await;
    wrap_as_responses_response(resp, &payload).await
}

fn responses_to_chat_completion(req: &ResponsesRequest) -> Result<ChatCompletionRequest, String> {
    let mut messages: Vec<ChatMessage> = Vec::new();

    if let Some(instructions) = &req.instructions {
        messages.push(ChatMessage {
            role: "system".to_string(),
            content: Some(MessageContent::Text(instructions.clone())),
            reasoning_content: None,
            name: None,
            tool_calls: None,
            tool_call_id: None,
        });
    }

    match &req.input {
        ResponsesInput::Text(text) => {
            messages.push(ChatMessage {
                role: "user".to_string(),
                content: Some(MessageContent::Text(text.clone())),
                reasoning_content: None,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }
        ResponsesInput::Items(items) => {
            for item in items {
                let item_type = item.item_type.as_deref().unwrap_or("message");
                match item_type {
                    "function_call_output" => {
                        let call_id = item.call_id.as_ref().ok_or("function_call_output requires call_id")?.clone();
                        let output = item.output.as_ref().ok_or("function_call_output requires output")?.clone();
                        messages.push(ChatMessage {
                            role: "tool".to_string(),
                            content: Some(MessageContent::Text(output)),
                            reasoning_content: None,
                            name: None,
                            tool_calls: None,
                            tool_call_id: Some(call_id),
                        });
                    }
                    "function_call" => {
                        let call_id = item.call_id.as_ref().ok_or("function_call requires call_id")?.clone();
                        let name = item.name.as_ref().ok_or("function_call requires name")?.clone();
                        let arguments = item.arguments.as_ref().ok_or("function_call requires arguments")?.clone();
                        messages.push(ChatMessage {
                            role: "assistant".to_string(),
                            content: Some(MessageContent::Text(String::new())),
                            reasoning_content: None,
                            name: None,
                            tool_calls: Some(vec![ToolCall {
                                id: call_id,
                                call_type: "function".to_string(),
                                function: ToolCallFunction {
                                    name,
                                    arguments,
                                },
                            }]),
                            tool_call_id: None,
                        });
                    }
                    _ => {
                        let role = item.role.as_deref().unwrap_or("user").to_string();
                        let content = item.content.clone().unwrap_or(MessageContent::Text(String::new()));
                        messages.push(ChatMessage {
                            role,
                            content: Some(content),
                            reasoning_content: None,
                            name: None,
                            tool_calls: None,
                            tool_call_id: None,
                        });
                    }
                }
            }
        }
    }

    Ok(ChatCompletionRequest {
        model: req.model.clone(),
        messages,
        stream: req.stream,
        temperature: req.temperature,
        top_p: req.top_p,
        max_tokens: req.max_output_tokens,
        tools: req.tools.clone(),
        tool_choice: req.tool_choice.clone(),
        reasoning_effort: None,
        thinking: None,
        metadata: req.metadata.clone(),
    })
}

async fn wrap_as_responses_response(inner_response: Response, req: &ResponsesRequest) -> Response {
    let (parts, body) = inner_response.into_parts();
    if !parts.status.is_success() {
        return Response::from_parts(parts, body);
    }

    let body_bytes = match axum::body::to_bytes(body, 10 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "response read error").into_response(),
    };

    let chat_resp: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(v) => v,
        Err(_) => return Response::from_parts(parts, Body::from(body_bytes)),
    };

    let mut output: Vec<ResponsesOutputItem> = Vec::new();

    if let Some(choices) = chat_resp.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            if let Some(msg) = choice.get("message") {
                if let Some(tool_calls_arr) = msg.get("tool_calls").and_then(|t| t.as_array()) {
                    for tc in tool_calls_arr {
                        if let (Some(id), Some(func)) = (tc.get("id").and_then(|v| v.as_str()), tc.get("function")) {
                            output.push(ResponsesOutputItem::FunctionCall {
                                id: format!("fc_{}", uuid::Uuid::new_v4()),
                                call_id: id.to_string(),
                                name: func.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                arguments: func.get("arguments").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                            });
                        }
                    }
                } else {
                    let text = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
                    output.push(ResponsesOutputItem::Message {
                        id: format!("msg_{}", uuid::Uuid::new_v4()),
                        role: "assistant",
                        content: vec![ResponsesContentPart {
                            part_type: "output_text",
                            text: text.to_string(),
                        }],
                    });
                }
            }
        }
    }

    let input_tokens_val = chat_resp.pointer("/usage/prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
    let output_tokens_val = chat_resp.pointer("/usage/completion_tokens").and_then(|v| v.as_i64()).unwrap_or(0) as i32;

    let responses_resp = ResponsesResponse {
        id: format!("resp_{}", uuid::Uuid::new_v4()),
        object: "response",
        created_at: chrono::Utc::now().timestamp(),
        model: req.model.clone(),
        output,
        previous_response_id: req.previous_response_id.clone(),
        usage: ResponsesUsage {
            input_tokens: input_tokens_val,
            output_tokens: output_tokens_val,
            total_tokens: input_tokens_val + output_tokens_val,
        },
    };

    (StatusCode::OK, Json(json!(responses_resp))).into_response()
}
