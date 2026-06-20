//! Kiro 端点抽象
//!
//! 不同 Kiro 端点在 URL、请求头、请求体上存在差异，
//! 但共享凭据池、Token 刷新、重试和响应解码逻辑。

use reqwest::RequestBuilder;

use crate::kiro::model::credentials::KiroCredentials;
use crate::model::config::Config;

pub mod cli;
pub mod ide;

pub use cli::{CLI_ENDPOINT_NAME, CliEndpoint};
pub use ide::{IDE_ENDPOINT_NAME, IDE_RUNTIME_ENDPOINT_NAME, IdeEndpoint};

pub struct UsageRequestParts {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
}

pub trait KiroEndpoint: Send + Sync {
    fn name(&self) -> &'static str;

    fn api_url(&self, ctx: &RequestContext<'_>) -> String;

    fn mcp_url(&self, ctx: &RequestContext<'_>) -> String;

    fn decorate_api(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder;

    fn decorate_mcp(&self, req: RequestBuilder, ctx: &RequestContext<'_>) -> RequestBuilder;

    fn transform_api_body(&self, body: &str, ctx: &RequestContext<'_>) -> anyhow::Result<String>;

    fn transform_mcp_body(&self, body: &str, _ctx: &RequestContext<'_>) -> anyhow::Result<String> {
        Ok(body.to_string())
    }

    fn usage_request_parts(&self, ctx: &RequestContext<'_>) -> anyhow::Result<UsageRequestParts>;

    fn is_monthly_request_limit(&self, body: &str) -> bool {
        default_is_monthly_request_limit(body)
    }

    fn is_bearer_token_invalid(&self, body: &str) -> bool {
        default_is_bearer_token_invalid(body)
    }

    fn is_account_throttled(&self, body: &str) -> bool {
        default_is_account_throttled(body)
    }

    fn is_client_validation_error(&self, body: &str) -> bool {
        default_is_client_validation_error(body)
    }

    fn is_gateway_timeout(&self, body: &str) -> bool {
        default_is_gateway_timeout(body)
    }
}

pub struct RequestContext<'a> {
    pub credentials: &'a KiroCredentials,
    pub token: &'a str,
    pub machine_id: &'a str,
    pub config: &'a Config,
}

pub fn default_is_monthly_request_limit(body: &str) -> bool {
    if body.contains("MONTHLY_REQUEST_COUNT") {
        return true;
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };

    if value
        .get("reason")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
    {
        return true;
    }

    value
        .pointer("/error/reason")
        .and_then(|v| v.as_str())
        .is_some_and(|v| v == "MONTHLY_REQUEST_COUNT")
}

pub fn default_is_bearer_token_invalid(body: &str) -> bool {
    body.contains("The bearer token included in the request is invalid")
}

pub fn default_is_account_throttled(body: &str) -> bool {
    body.contains("suspicious activity") && body.contains("temporary limits")
}

pub fn default_is_gateway_timeout(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    body.contains("524")
        && (lower.contains("status code")
            || lower.contains("gateway timeout")
            || lower.contains("server-side issue"))
}

const CLIENT_VALIDATION_REASONS: &[&str] = &[
    "INVALID_TOOL_CALL_ID",
    "MISSING_TOOL_RESULT",
    "DUPLICATE_TOOL_RESULT",
    "ORPHANED_TOOL_RESULT",
    "INVALID_MESSAGE_ORDERING",
];

pub fn default_is_client_validation_error(body: &str) -> bool {
    let reason_hit = CLIENT_VALIDATION_REASONS.iter().any(|r| body.contains(r));
    if reason_hit {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
            let top = value.get("reason").and_then(|v| v.as_str());
            let nested = value.pointer("/error/reason").and_then(|v| v.as_str());
            if [top, nested]
                .into_iter()
                .flatten()
                .any(|r| CLIENT_VALIDATION_REASONS.contains(&r))
            {
                return true;
            }
        } else {
            return true;
        }
    }
    false
}
