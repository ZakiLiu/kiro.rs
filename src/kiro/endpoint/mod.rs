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
pub use ide::{IDE_ENDPOINT_NAME, IdeEndpoint};

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
}

pub struct RequestContext<'a> {
    pub credentials: &'a KiroCredentials,
    pub token: &'a str,
    pub machine_id: &'a str,
    pub config: &'a Config,
}

const QUOTA_LIMIT_REASONS: &[&str] = &[
    "MONTHLY_REQUEST_COUNT",
    "OVERAGE_REQUEST_LIMIT_EXCEEDED",
];

pub fn default_is_monthly_request_limit(body: &str) -> bool {
    if QUOTA_LIMIT_REASONS.iter().any(|r| body.contains(r)) {
        return true;
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return false;
    };

    let check_reason = |v: Option<&serde_json::Value>| {
        v.and_then(|v| v.as_str())
            .is_some_and(|r| QUOTA_LIMIT_REASONS.iter().any(|q| *q == r))
    };

    check_reason(value.get("reason")) || check_reason(value.pointer("/error/reason"))
}

pub fn default_is_bearer_token_invalid(body: &str) -> bool {
    body.contains("The bearer token included in the request is invalid")
}
