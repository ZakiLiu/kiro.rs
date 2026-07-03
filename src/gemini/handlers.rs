//! Gemini 兼容 API Handler

use std::convert::Infallible;

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::json;
use std::time::Duration;
use tokio::time::{Instant, interval_at};

use crate::anthropic::handlers::{
    TelemetryData, last_attempt_credential_id, record_request_telemetry,
};
use crate::anthropic::middleware::{AppState, AuthIdentity};
use crate::kiro::model::events::Event;
use crate::kiro::parser::decoder::EventStreamDecoder;
use crate::token;

use super::converter::convert_gemini_to_kiro;
use super::types::*;

pub async fn generate_content(
    State(state): State<AppState>,
    axum::Extension(auth): axum::Extension<AuthIdentity>,
    Path(model_action): Path<String>,
    Json(payload): Json<GenerateContentRequest>,
) -> impl IntoResponse {
    let model = model_action
        .strip_suffix(":generateContent")
        .or_else(|| model_action.strip_suffix(":streamGenerateContent"))
        .unwrap_or(&model_action);

    tracing::info!(model = %model, "Gemini /v1beta/models generateContent");

    let provider = match &state.kiro_provider {
        Some(p) => p.clone(),
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": {"code": 503, "message": "服务未就绪", "status": "UNAVAILABLE"}})),
            )
                .into_response();
        }
    };

    let is_stream = model_action.contains(":streamGenerateContent");

    let conversion = match convert_gemini_to_kiro(&payload, model, state.profile_arn.clone()) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": {"code": 400, "message": e, "status": "INVALID_ARGUMENT"}})),
            )
                .into_response();
        }
    };

    let request_body = match serde_json::to_string(&conversion.kiro_request) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": {"code": 500, "message": format!("序列化失败: {}", e), "status": "INTERNAL"}})),
            )
                .into_response();
        }
    };

    let input_tokens = token::count_tokens(&request_body) as i32;

    if is_stream {
        handle_stream(provider, &request_body, model, input_tokens, &state, &auth).await
    } else {
        handle_non_stream(provider, &request_body, model, input_tokens, &state, &auth).await
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
    let result = match provider
        .call_api_stream(request_body, None, auth.group.as_deref())
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let crate::kiro::provider::ApiCallError { error, attempts } = e;
            let error_message = error.to_string();
            tracing::error!("Gemini upstream stream error: {}", error);
            record_request_telemetry(
                state,
                auth,
                TelemetryData {
                    model,
                    is_stream: true,
                    credential_id: last_attempt_credential_id(&attempts),
                    input_tokens,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    credits: 0.0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    status: "error",
                    attempts,
                    first_token_ms: None,
                    error_message: Some(error_message),
                },
            );
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"code": 502, "message": "upstream service error", "status": "UNAVAILABLE"}})),
            )
                .into_response();
        }
    };

    let stream = create_gemini_sse_stream(
        result,
        model.to_string(),
        input_tokens,
        state.clone(),
        auth.clone(),
        start,
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("Connection", "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| (StatusCode::INTERNAL_SERVER_ERROR, "stream error").into_response())
}

fn create_gemini_sse_stream(
    api_result: crate::kiro::provider::ApiCallResult,
    model: String,
    input_tokens: i32,
    state: AppState,
    auth: AuthIdentity,
    start: Instant,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let credential_id = api_result.credential_id;
    let attempts = api_result.attempts;
    let response = api_result.response;
    let _model = model.clone();

    async_stream::stream! {
        let mut decoder = EventStreamDecoder::new();
        let mut byte_stream = response.bytes_stream();
        let mut output_tokens = 0i32;
        let mut final_input_tokens = input_tokens;
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
                                            Ok(Event::AssistantResponse(ev)) => {
                                                if !ev.content.is_empty() {
                                                    output_tokens += token::count_tokens(&ev.content) as i32;
                                                    let chunk = GenerateContentResponse {
                                                        candidates: vec![Candidate {
                                                            content: ResponseContent {
                                                                parts: vec![ResponsePart { text: ev.content }],
                                                                role: "model".to_string(),
                                                            },
                                                            finish_reason: None,
                                                        }],
                                                        usage_metadata: None,
                                                    };
                                                    let json = serde_json::to_string(&chunk).unwrap_or_default();
                                                    yield Ok(Bytes::from(format!("data: {}\n\n", json)));
                                                }
                                            }
                                            Ok(Event::TokenUsage(ev)) => {
                                                final_input_tokens = ev.uncached_input_tokens as i32;
                                                output_tokens = ev.output_tokens as i32;
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

        let final_chunk = GenerateContentResponse {
            candidates: vec![Candidate {
                content: ResponseContent {
                    parts: vec![ResponsePart { text: String::new() }],
                    role: "model".to_string(),
                },
                finish_reason: Some("STOP".to_string()),
            }],
            usage_metadata: Some(UsageMetadata {
                prompt_token_count: final_input_tokens,
                candidates_token_count: output_tokens,
                total_token_count: final_input_tokens + output_tokens,
            }),
        };
        let json = serde_json::to_string(&final_chunk).unwrap_or_default();
        yield Ok(Bytes::from(format!("data: {}\n\ndata: [DONE]\n\n", json)));

        let duration_ms = start.elapsed().as_millis() as u64;
        record_request_telemetry(
            &state, &auth, TelemetryData {
                model: &model,
                is_stream: true,
                credential_id,
                input_tokens: final_input_tokens,
                output_tokens,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
                credits: 0.0,
                duration_ms,
                status: "success",
                attempts,
                first_token_ms: None,
                error_message: None,
            },
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
    let result = match provider
        .call_api(request_body, None, auth.group.as_deref())
        .await
    {
        Ok(r) => r,
        Err(e) => {
            let crate::kiro::provider::ApiCallError { error, attempts } = e;
            let error_message = error.to_string();
            tracing::error!("Gemini upstream error: {}", error);
            record_request_telemetry(
                state,
                auth,
                TelemetryData {
                    model,
                    is_stream: false,
                    credential_id: last_attempt_credential_id(&attempts),
                    input_tokens,
                    output_tokens: 0,
                    cache_creation_tokens: 0,
                    cache_read_tokens: 0,
                    credits: 0.0,
                    duration_ms: start.elapsed().as_millis() as u64,
                    status: "error",
                    attempts,
                    first_token_ms: None,
                    error_message: Some(error_message),
                },
            );
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": {"code": 502, "message": "upstream service error", "status": "UNAVAILABLE"}})),
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
                Json(json!({"error": {"code": 502, "message": format!("读取响应失败: {}", e), "status": "UNAVAILABLE"}})),
            )
                .into_response();
        }
    };

    let mut decoder = EventStreamDecoder::new();
    if let Err(e) = decoder.feed(&response_bytes) {
        return (
            StatusCode::BAD_GATEWAY,
            Json(json!({"error": {"code": 502, "message": format!("decoder feed error: {}", e), "status": "UNAVAILABLE"}})),
        )
            .into_response();
    }

    let mut text_content = String::new();
    let mut output_tokens = 0i32;
    let mut final_input_tokens = input_tokens;

    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => match Event::from_frame(frame) {
                Ok(Event::AssistantResponse(ev)) => {
                    if !ev.content.is_empty() {
                        text_content.push_str(&ev.content);
                        output_tokens += token::count_tokens(&ev.content) as i32;
                    }
                }
                Ok(Event::TokenUsage(ev)) => {
                    final_input_tokens = ev.uncached_input_tokens as i32;
                    output_tokens = ev.output_tokens as i32;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("event parse error: {}", e);
                }
            },
            Err(e) => {
                tracing::warn!("decode error: {}", e);
            }
        }
    }

    let response = GenerateContentResponse {
        candidates: vec![Candidate {
            content: ResponseContent {
                parts: vec![ResponsePart { text: text_content }],
                role: "model".to_string(),
            },
            finish_reason: Some("STOP".to_string()),
        }],
        usage_metadata: Some(UsageMetadata {
            prompt_token_count: final_input_tokens,
            candidates_token_count: output_tokens,
            total_token_count: final_input_tokens + output_tokens,
        }),
    };

    let duration_ms = start.elapsed().as_millis() as u64;
    record_request_telemetry(
        state,
        auth,
        TelemetryData {
            model,
            is_stream: false,
            credential_id,
            input_tokens: final_input_tokens,
            output_tokens,
            cache_creation_tokens: 0,
            cache_read_tokens: 0,
            credits: 0.0,
            duration_ms,
            status: "success",
            attempts,
            first_token_ms: None,
            error_message: None,
        },
    );

    (StatusCode::OK, Json(json!(response))).into_response()
}

pub async fn list_models(State(_state): State<AppState>) -> impl IntoResponse {
    let models = vec![
        GeminiModel {
            name: "models/gemini-2.5-pro".to_string(),
            display_name: "Gemini 2.5 Pro".to_string(),
            input_token_limit: 1048576,
            output_token_limit: 65536,
            supported_generation_methods: vec![
                "generateContent".to_string(),
                "streamGenerateContent".to_string(),
            ],
        },
        GeminiModel {
            name: "models/gemini-2.5-flash".to_string(),
            display_name: "Gemini 2.5 Flash".to_string(),
            input_token_limit: 1048576,
            output_token_limit: 65536,
            supported_generation_methods: vec![
                "generateContent".to_string(),
                "streamGenerateContent".to_string(),
            ],
        },
        GeminiModel {
            name: "models/gemini-1.5-pro".to_string(),
            display_name: "Gemini 1.5 Pro".to_string(),
            input_token_limit: 2097152,
            output_token_limit: 8192,
            supported_generation_methods: vec![
                "generateContent".to_string(),
                "streamGenerateContent".to_string(),
            ],
        },
        GeminiModel {
            name: "models/gemini-1.5-flash".to_string(),
            display_name: "Gemini 1.5 Flash".to_string(),
            input_token_limit: 1048576,
            output_token_limit: 8192,
            supported_generation_methods: vec![
                "generateContent".to_string(),
                "streamGenerateContent".to_string(),
            ],
        },
    ];

    (StatusCode::OK, Json(json!(GeminiModelsResponse { models }))).into_response()
}
