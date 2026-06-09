//! 错误分类与 Anthropic 格式响应生成模块
//!
//! 将上游 / provider 层的 `anyhow::Error` 统一分类为 [`ErrorCategory`]，
//! 并通过 [`to_anthropic_response`] 生成符合 Anthropic API 格式的 HTTP 响应。
//!
//! 本模块取代 `handlers.rs` 中散落的 8 个谓词函数和单体 `map_kiro_provider_error_to_response`。
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::anthropic::types::ErrorResponse;

// ── ErrorCategory ──────────────────────────────────────────────────

/// 错误分类枚举
///
/// 每个变体对应一种可辨识的上游错误类型，1:1 映射 HTTP 状态码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorCategory {
    /// 输入上下文过长（CONTENT_LENGTH_EXCEEDS_THRESHOLD / Input is too long）
    InputTooLong,

    /// 请求格式错误（Improperly formed request，非压缩引起）
    ImproperlyFormedRequest,

    /// 压缩后引起的 400（was_compressed && Improperly formed request）
    CompressionInduced400,

    /// 所有凭据配额耗尽
    QuotaExhausted,

    /// 无可用凭据
    NoCredentials,

    /// 所有凭据处于冷却/速率限制
    AllCredentialsCooling {
        /// 建议的重试等待秒数
        retry_after_secs: Option<u64>,
    },

    /// 上游速率限制或瞬态 429/5xx（不含网络错误）
    RateLimitTransient,

    /// 网络层瞬态错误（连接重置/关闭/发送失败）
    NetworkTransient,

    /// 模型暂时不可用（MODEL_TEMPORARILY_UNAVAILABLE 全局熔断）
    ModelUnavailable,

    /// 认证失败（401/403，当前由 middleware 处理，预留）
    #[allow(dead_code)]
    AuthFailure,

    /// 服务器瞬态错误（兜底 5xx）
    #[allow(dead_code)]
    ServerTransient,

    /// 无法识别的错误
    Unknown,
}

// ── ErrorRequestContext ────────────────────────────────────────────

/// 错误请求上下文
///
/// 携带压缩诊断信息，用于区分 `ImproperlyFormedRequest` 与 `CompressionInduced400`。
#[derive(Debug, Clone, Default)]
pub struct ErrorRequestContext {
    /// 请求是否经过压缩处理
    pub was_compressed: bool,

    /// 原始请求体大小（bytes）
    pub request_body_bytes: usize,

    /// 压缩迭代次数（若未压缩则为 None）
    /// 仅在 `sensitive-logs` feature 启用时用于诊断日志
    #[cfg_attr(not(feature = "sensitive-logs"), allow(dead_code))]
    pub compression_iterations: Option<usize>,
}

// ── 网络错误关键字 ──────────────────────────────────────────────────

/// 网络错误关键字（与 handlers.rs NETWORK_ERROR_PATTERNS 保持一致）
const NETWORK_ERROR_PATTERNS: &[&str] = &[
    "error sending request",
    "connection closed",
    "connection reset",
];

/// 检查字符串是否包含网络错误关键字
fn is_network_error(s: &str) -> bool {
    NETWORK_ERROR_PATTERNS.iter().any(|p| s.contains(p))
}

// ── classify ───────────────────────────────────────────────────────

/// 将 `anyhow::Error` 分类为 [`ErrorCategory`]
///
/// 匹配顺序与 `handlers.rs` 中 `map_kiro_provider_error_to_response` 一致：
/// 先匹配最具体的模式，再到兜底。
pub fn classify(err: &anyhow::Error, ctx: &ErrorRequestContext) -> ErrorCategory {
    let s = err.to_string();

    // 1. 输入过长（确定性，不应重试）
    if s.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") || s.contains("Input is too long") {
        return ErrorCategory::InputTooLong;
    }

    // 2. 请求格式错误 — 区分压缩引起和非压缩引起
    if s.contains("Improperly formed request") {
        return if ctx.was_compressed {
            ErrorCategory::CompressionInduced400
        } else {
            ErrorCategory::ImproperlyFormedRequest
        };
    }

    // 3a. 账户暂停（403 suspended → 归入 NoCredentials，返回 503 而非 500）
    if s.contains("账户暂停") || s.contains("suspended") {
        return ErrorCategory::NoCredentials;
    }

    // 3b. 无可用凭据
    if s.contains("没有可用的凭据") {
        return ErrorCategory::NoCredentials;
    }

    // 4. 所有凭据冷却中（提取 retry_after_secs）
    if s.contains("所有凭据均处于冷却/速率限制") {
        let retry = s.split("retry_after_secs=").nth(1).and_then(|rest| {
            let end = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            rest[..end].parse::<u64>().ok()
        });
        return ErrorCategory::AllCredentialsCooling {
            retry_after_secs: retry,
        };
    }

    // 5. 配额耗尽
    if s.contains("所有凭据已用尽")
        || s.contains("OVERAGE_REQUEST_LIMIT_EXCEEDED")
        || s.contains("MONTHLY_REQUEST_COUNT")
    {
        return ErrorCategory::QuotaExhausted;
    }

    // 6. 模型暂时不可用
    if s.contains("MODEL_TEMPORARILY_UNAVAILABLE") {
        return ErrorCategory::ModelUnavailable;
    }

    // 7. 瞬态上游错误（429 / 5xx / 网络）
    let lower = s.to_lowercase();
    if lower.contains("429 too many requests")
        || lower.contains("insufficient_model_capacity")
        || lower.contains("high traffic")
        || lower.contains("408 request timeout")
        || lower.contains("502 bad gateway")
        || lower.contains("503 service unavailable")
        || lower.contains("504 gateway timeout")
        || is_network_error(&lower)
    {
        // 网络错误细分为 NetworkTransient
        if is_network_error(&lower) {
            return ErrorCategory::NetworkTransient;
        }
        return ErrorCategory::RateLimitTransient;
    }

    ErrorCategory::Unknown
}

// ── to_anthropic_response ──────────────────────────────────────────

/// 根据 [`ErrorCategory`] 生成 Anthropic 格式的 HTTP 响应
///
/// 状态码映射：
/// - `InputTooLong` / `ImproperlyFormedRequest` / `CompressionInduced400` → 400
/// - `QuotaExhausted` / `AllCredentialsCooling` / `RateLimitTransient` → 429
/// - `NoCredentials` / `ModelUnavailable` → 503
/// - `NetworkTransient` / `ServerTransient` → 502
/// - `AuthFailure` → 401
/// - `Unknown` → 500
pub fn to_anthropic_response(
    category: &ErrorCategory,
    err: &anyhow::Error,
    ctx: &ErrorRequestContext,
) -> Response {
    match category {
        ErrorCategory::InputTooLong => {
            tracing::warn!(
                kiro_request_body_bytes = ctx.request_body_bytes,
                error = %err,
                "上游拒绝请求：输入上下文过长（不应重试）"
            );
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(
                    "invalid_request_error",
                    "Input is too long (CONTENT_LENGTH_EXCEEDS_THRESHOLD). Reduce conversation history/system/tools; retrying the same request will not help.",
                )),
            )
                .into_response()
        }

        ErrorCategory::ImproperlyFormedRequest => {
            tracing::warn!(
                error = %err,
                kiro_request_body_bytes = ctx.request_body_bytes,
                "上游拒绝请求：请求格式错误（可能由超大请求体、消息/工具序列异常或空内容块导致）"
            );
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(
                    "invalid_request_error",
                    "Improperly formed request. This is often caused by oversized payloads, malformed message/tool sequences, or empty content blocks.",
                )),
            )
                .into_response()
        }

        ErrorCategory::CompressionInduced400 => {
            tracing::warn!(
                error = %err,
                kiro_request_body_bytes = ctx.request_body_bytes,
                "压缩后上游仍返回 400：压缩可能破坏了消息结构"
            );
            #[cfg(feature = "sensitive-logs")]
            tracing::warn!(
                request_body_bytes = ctx.request_body_bytes,
                compression_iterations = ?ctx.compression_iterations,
                was_compressed = ctx.was_compressed,
                "CompressionInduced400 诊断：压缩迭代 {:?} 次，请求体 {} bytes",
                ctx.compression_iterations,
                ctx.request_body_bytes,
            );
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(
                    "invalid_request_error",
                    "Improperly formed request. This may have been caused by input compression altering the message structure. Try reducing input size directly.",
                )),
            )
                .into_response()
        }

        ErrorCategory::NoCredentials => {
            tracing::error!(error = %err, "没有可用的凭据");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "No credentials available. Please add or enable credentials via Admin API or credentials.json.",
                )),
            )
                .into_response()
        }

        ErrorCategory::AllCredentialsCooling { retry_after_secs } => {
            // 下限从 60s 降到 5s：当配置了低 RPM 时，实际等待可能只需几秒，
            // 硬钳到 60s 会让客户端白等。上限 300s 保持不变。
            let secs = retry_after_secs.unwrap_or(60).clamp(5, 300);
            tracing::warn!(
                error = %err,
                retry_after_secs = secs,
                "所有凭据临时冷却，返回 429 + Retry-After"
            );
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, secs.to_string())],
                Json(ErrorResponse::new(
                    "rate_limit_error",
                    format!(
                        "All credentials are temporarily cooling down. Retry after {}s.",
                        secs
                    ),
                )),
            )
                .into_response()
        }

        ErrorCategory::QuotaExhausted => {
            tracing::warn!(error = %err, "所有凭据配额已耗尽");
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse::new(
                    "rate_limit_error",
                    "All credentials quota exhausted. Please wait for quota reset or add new credentials.",
                )),
            )
                .into_response()
        }

        ErrorCategory::RateLimitTransient => {
            tracing::warn!(error = %err, "上游瞬态错误（429/5xx），不输出请求体");
            (
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse::new("rate_limit_error", "Rate limited by upstream. Please retry after backoff.")),
            )
                .into_response()
        }

        ErrorCategory::NetworkTransient => {
            tracing::warn!(error = %err, "上游网络错误，不输出请求体");
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    "Upstream network error. Please retry.",
                )),
            )
                .into_response()
        }

        ErrorCategory::ModelUnavailable => {
            tracing::warn!(error = %err, "模型暂时不可用（全局熔断）");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse::new(
                    "service_unavailable",
                    "Model temporarily unavailable (MODEL_TEMPORARILY_UNAVAILABLE). Please retry later.",
                )),
            )
                .into_response()
        }

        ErrorCategory::AuthFailure => {
            tracing::warn!(error = %err, "认证失败");
            (
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse::authentication_error()),
            )
                .into_response()
        }

        ErrorCategory::ServerTransient => {
            tracing::error!(error = %err, "上游服务器瞬态错误");
            #[cfg(feature = "sensitive-logs")]
            tracing::error!(
                request_body_bytes = ctx.request_body_bytes,
                "上游报错，请求体大小: {} bytes",
                ctx.request_body_bytes,
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    "Upstream API error. Please retry.",
                )),
            )
                .into_response()
        }

        ErrorCategory::Unknown => {
            tracing::error!("Kiro API 调用失败: {}", err);
            #[cfg(feature = "sensitive-logs")]
            tracing::error!(
                request_body_bytes = ctx.request_body_bytes,
                "上游报错，请求体大小: {} bytes",
                ctx.request_body_bytes,
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::new(
                    "api_error",
                    "Upstream API error. Please retry.",
                )),
            )
                .into_response()
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn make_err(msg: &str) -> anyhow::Error {
        anyhow::anyhow!("{}", msg)
    }

    fn default_ctx() -> ErrorRequestContext {
        ErrorRequestContext::default()
    }

    fn compressed_ctx() -> ErrorRequestContext {
        ErrorRequestContext {
            was_compressed: true,
            request_body_bytes: 4_000_000,
            compression_iterations: Some(3),
        }
    }

    // ── classify tests ─────────────────────────────────────────────

    #[test]
    fn test_classify_input_too_long_content_length() {
        let err = make_err("upstream error: CONTENT_LENGTH_EXCEEDS_THRESHOLD");
        assert_eq!(classify(&err, &default_ctx()), ErrorCategory::InputTooLong);
    }

    #[test]
    fn test_classify_input_too_long_input_is_too_long() {
        let err = make_err("Input is too long for this model");
        assert_eq!(classify(&err, &default_ctx()), ErrorCategory::InputTooLong);
    }

    #[test]
    fn test_classify_improperly_formed_request() {
        let err = make_err("Improperly formed request body");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::ImproperlyFormedRequest
        );
    }

    #[test]
    fn test_classify_compression_induced_400() {
        let err = make_err("Improperly formed request after compression");
        let ctx = compressed_ctx();
        assert_eq!(
            classify(&err, &ctx),
            ErrorCategory::CompressionInduced400
        );
    }

    #[test]
    fn test_classify_quota_exhausted() {
        let err = make_err("所有凭据已用尽，无法继续");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::QuotaExhausted
        );
    }

    #[test]
    fn test_classify_no_credentials() {
        let err = make_err("没有可用的凭据，请添加凭据");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::NoCredentials
        );
    }

    #[test]
    fn test_classify_all_cooling_down_with_retry() {
        let err = make_err("所有凭据均处于冷却/速率限制中 retry_after_secs=120");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::AllCredentialsCooling {
                retry_after_secs: Some(120)
            }
        );
    }

    #[test]
    fn test_classify_all_cooling_down_without_retry() {
        let err = make_err("所有凭据均处于冷却/速率限制中");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::AllCredentialsCooling {
                retry_after_secs: None
            }
        );
    }

    #[test]
    fn test_classify_rate_limit_transient_429() {
        let err = make_err("upstream returned 429 Too Many Requests");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::RateLimitTransient
        );
    }

    #[test]
    fn test_classify_rate_limit_transient_insufficient_capacity() {
        let err = make_err("insufficient_model_capacity");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::RateLimitTransient
        );
    }

    #[test]
    fn test_classify_rate_limit_transient_high_traffic() {
        let err = make_err("high traffic detected, please retry");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::RateLimitTransient
        );
    }

    #[test]
    fn test_classify_rate_limit_transient_5xx() {
        for code in &["502 Bad Gateway", "503 Service Unavailable", "504 Gateway Timeout", "408 Request Timeout"] {
            let err = make_err(code);
            assert_eq!(
                classify(&err, &default_ctx()),
                ErrorCategory::RateLimitTransient,
                "expected RateLimitTransient for '{}'",
                code
            );
        }
    }

    #[test]
    fn test_classify_network_error() {
        for pattern in &["error sending request to upstream", "connection closed by peer", "connection reset by remote"] {
            let err = make_err(pattern);
            assert_eq!(
                classify(&err, &default_ctx()),
                ErrorCategory::NetworkTransient,
                "expected NetworkTransient for '{}'",
                pattern
            );
        }
    }

    #[test]
    fn test_classify_model_unavailable() {
        let err = make_err("MODEL_TEMPORARILY_UNAVAILABLE: please wait");
        assert_eq!(
            classify(&err, &default_ctx()),
            ErrorCategory::ModelUnavailable
        );
    }

    #[test]
    fn test_classify_unknown() {
        let err = make_err("some completely unexpected error");
        assert_eq!(classify(&err, &default_ctx()), ErrorCategory::Unknown);
    }

    // ── to_anthropic_response status code tests ────────────────────

    #[tokio::test]
    async fn test_response_status_codes() {
        let cases: Vec<(ErrorCategory, StatusCode)> = vec![
            (ErrorCategory::InputTooLong, StatusCode::BAD_REQUEST),
            (ErrorCategory::ImproperlyFormedRequest, StatusCode::BAD_REQUEST),
            (ErrorCategory::CompressionInduced400, StatusCode::BAD_REQUEST),
            (ErrorCategory::QuotaExhausted, StatusCode::TOO_MANY_REQUESTS),
            (ErrorCategory::NoCredentials, StatusCode::SERVICE_UNAVAILABLE),
            (
                ErrorCategory::AllCredentialsCooling {
                    retry_after_secs: Some(120),
                },
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (ErrorCategory::RateLimitTransient, StatusCode::TOO_MANY_REQUESTS),
            (ErrorCategory::NetworkTransient, StatusCode::BAD_GATEWAY),
            (ErrorCategory::ModelUnavailable, StatusCode::SERVICE_UNAVAILABLE),
            (ErrorCategory::AuthFailure, StatusCode::UNAUTHORIZED),
            (ErrorCategory::ServerTransient, StatusCode::BAD_GATEWAY),
            (ErrorCategory::Unknown, StatusCode::INTERNAL_SERVER_ERROR),
        ];

        let err = make_err("test error");
        let ctx = default_ctx();

        for (category, expected_status) in cases {
            let resp = to_anthropic_response(&category, &err, &ctx);
            assert_eq!(
                resp.status(),
                expected_status,
                "category {:?} should return {}",
                category,
                expected_status
            );
        }
    }

    #[tokio::test]
    async fn test_response_retry_after_header() {
        let err = make_err("所有凭据均处于冷却/速率限制中 retry_after_secs=120");
        let ctx = default_ctx();
        let category = ErrorCategory::AllCredentialsCooling {
            retry_after_secs: Some(120),
        };

        let resp = to_anthropic_response(&category, &err, &ctx);
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);

        let retry_after = resp.headers().get(header::RETRY_AFTER).unwrap();
        assert_eq!(retry_after.to_str().unwrap(), "120");
    }

    #[tokio::test]
    async fn test_response_retry_after_clamping_low() {
        let err = make_err("所有凭据均处于冷却/速率限制中 retry_after_secs=3");
        let ctx = default_ctx();
        let category = ErrorCategory::AllCredentialsCooling {
            retry_after_secs: Some(3),
        };

        let resp = to_anthropic_response(&category, &err, &ctx);
        let retry_after = resp.headers().get(header::RETRY_AFTER).unwrap();
        // 应当 clamp 到最小 5
        assert_eq!(retry_after.to_str().unwrap(), "5");
    }

    #[tokio::test]
    async fn test_response_retry_after_clamping_high() {
        let err = make_err("所有凭据均处于冷却/速率限制中 retry_after_secs=999");
        let ctx = default_ctx();
        let category = ErrorCategory::AllCredentialsCooling {
            retry_after_secs: Some(999),
        };

        let resp = to_anthropic_response(&category, &err, &ctx);
        let retry_after = resp.headers().get(header::RETRY_AFTER).unwrap();
        // 应当 clamp 到最大 300
        assert_eq!(retry_after.to_str().unwrap(), "300");
    }

    #[tokio::test]
    async fn test_response_retry_after_default_when_none() {
        let err = make_err("所有凭据均处于冷却/速率限制中");
        let ctx = default_ctx();
        let category = ErrorCategory::AllCredentialsCooling {
            retry_after_secs: None,
        };

        let resp = to_anthropic_response(&category, &err, &ctx);
        let retry_after = resp.headers().get(header::RETRY_AFTER).unwrap();
        // None → default 60, clamp [5, 300] → 60
        assert_eq!(retry_after.to_str().unwrap(), "60");
    }

    #[tokio::test]
    async fn test_response_body_is_valid_error_json() {
        let err = make_err("Input is too long");
        let ctx = default_ctx();
        let category = ErrorCategory::InputTooLong;

        let resp = to_anthropic_response(&category, &err, &ctx);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert!(json.get("error").is_some());
        assert_eq!(json["error"]["type"], "invalid_request_error");
        assert!(json["error"]["message"].as_str().unwrap().contains("Input is too long"));
    }

    #[tokio::test]
    async fn test_no_retry_after_for_non_cooling() {
        let err = make_err("some rate limit error");
        let ctx = default_ctx();

        // QuotaExhausted 不应有 Retry-After
        let resp = to_anthropic_response(&ErrorCategory::QuotaExhausted, &err, &ctx);
        assert!(resp.headers().get(header::RETRY_AFTER).is_none());

        // RateLimitTransient 不应有 Retry-After
        let resp = to_anthropic_response(&ErrorCategory::RateLimitTransient, &err, &ctx);
        assert!(resp.headers().get(header::RETRY_AFTER).is_none());
    }

    // ── ErrorRequestContext tests ──────────────────────────────────

    #[test]
    fn test_error_request_context_default() {
        let ctx = ErrorRequestContext::default();
        assert!(!ctx.was_compressed);
        assert_eq!(ctx.request_body_bytes, 0);
        assert!(ctx.compression_iterations.is_none());
    }
}
