//! 公共认证工具函数

use axum::{
    body::Body,
    http::{Request, header},
};
use subtle::ConstantTimeEq;

/// 从请求中提取 API Key
///
/// 支持两种认证方式：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn extract_api_key(request: &Request<Body>) -> Option<String> {
    // 优先检查 x-api-key
    if let Some(key) = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
    {
        return Some(key.to_string());
    }

    // 其次检查 Authorization: Bearer
    request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

/// 常量时间字符串比较，防止时序攻击
///
/// 无论字符串内容如何，比较所需的时间都是恒定的，
/// 这可以防止攻击者通过测量响应时间来猜测 API Key。
///
/// 使用经过安全审计的 `subtle` crate 实现。
/// 已知限制：ct_eq 对内容常量时间，但长度不等时返回 false 的耗时可暴露 key 长度。
/// 风险低（仅泄露长度，不泄露内容），如需加固可先 SHA-256 再比较。
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
