use anyhow::bail;
use chrono::{Duration, Utc};
use url::Url;

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::token_refresh::ExternalIdpRefreshResponse;
use crate::model::config::Config;

pub const ALLOWED_IDP_HOST_SUFFIXES: &[&str] = &[
    "login.microsoftonline.com",
    "login.microsoftonline.us",
    "login.chinacloudapi.cn",
    "login.microsoftonline.de",
];

pub fn validate_external_idp_endpoint(
    url_str: &str,
    allowed_suffixes: &[&str],
) -> Result<(), String> {
    let url = Url::parse(url_str).map_err(|e| format!("invalid URL: {e}"))?;

    if url.scheme() != "https" {
        return Err(format!(
            "external IdP endpoint rejected: scheme must be https, got '{}'",
            url.scheme()
        ));
    }

    let host = url
        .host_str()
        .ok_or("external IdP endpoint rejected: no host")?;

    if host.parse::<std::net::Ipv4Addr>().is_ok()
        || host.starts_with('[')
        || host.parse::<std::net::Ipv6Addr>().is_ok()
    {
        return Err(format!(
            "external IdP endpoint rejected: IP literal not allowed: {host}"
        ));
    }

    let host_lower = host.to_ascii_lowercase();
    let matched = allowed_suffixes.iter().any(|suffix| {
        host_lower == *suffix || host_lower.ends_with(&format!(".{suffix}"))
    });

    if !matched {
        return Err(format!(
            "external IdP endpoint rejected: host '{host}' not in allow-list"
        ));
    }

    Ok(())
}

pub fn validate_with_default_allowlist(url_str: &str) -> Result<(), String> {
    validate_external_idp_endpoint(url_str, ALLOWED_IDP_HOST_SUFFIXES)
}

/// External IdP Token 刷新（OIDC refresh_token grant, form-urlencoded POST）
pub async fn refresh_external_idp_token(
    credentials: &KiroCredentials,
    config: &Config,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<KiroCredentials> {
    tracing::info!("正在刷新 External IdP Token...");

    let refresh_token = credentials
        .refresh_token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("external_idp 刷新需要 refreshToken"))?;
    let client_id = credentials
        .client_id
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("external_idp 刷新需要 clientId"))?;
    let token_endpoint = credentials
        .token_endpoint
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("external_idp 刷新需要 tokenEndpoint"))?;

    // SSRF 重验证（SA-06: refresh POST 边界也要检查）
    validate_with_default_allowlist(token_endpoint)
        .map_err(|e| anyhow::anyhow!("external_idp refresh blocked: {e}"))?;

    let client = build_client(proxy, 60, config.tls_backend)?;

    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("client_id", client_id.as_str()),
        ("refresh_token", refresh_token.as_str()),
    ];

    // scope 仅在凭据含 scopes 时携带
    let scopes_val;
    if let Some(s) = credentials.scopes.as_deref() {
        if !s.trim().is_empty() {
            scopes_val = s.to_string();
            form.push(("scope", &scopes_val));
        }
    }

    let response = client
        .post(token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let error_msg = match status.as_u16() {
            400 if body_text.contains("invalid_grant") => {
                "external_idp 凭证已过期或无效 (invalid_grant)，需要重新认证"
            }
            401 => "external_idp 凭证认证失败",
            403 => "权限不足，无法刷新 external_idp Token",
            429 => "请求过于频繁，已被限流",
            500..=599 => "IdP 服务器错误，外部身份提供商暂时不可用",
            _ => "external_idp Token 刷新失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let data: ExternalIdpRefreshResponse = response.json().await?;

    let mut new_credentials = credentials.clone();
    new_credentials.access_token = Some(data.access_token);

    if let Some(new_refresh_token) = data.refresh_token {
        new_credentials.refresh_token = Some(new_refresh_token);
    }

    if let Some(expires_in) = data.expires_in {
        let expires_at = Utc::now() + Duration::seconds(expires_in);
        new_credentials.expires_at = Some(expires_at.to_rfc3339());
        tracing::info!(expires_in = %expires_in, "External IdP Token 刷新成功");
    } else {
        tracing::info!("External IdP Token 刷新成功（无过期时间）");
    }

    Ok(new_credentials)
}

/// 解析 JWT access_token 的 exp 字段（Base64URL 无填充解码，不验签）
pub fn parse_jwt_exp(access_token: &str) -> Option<i64> {
    let parts: Vec<&str> = access_token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }

    let payload = parts[1];
    // Base64URL 无填充 → 标准 Base64 需要补 padding
    let padded = match payload.len() % 4 {
        2 => format!("{payload}=="),
        3 => format!("{payload}="),
        _ => payload.to_string(),
    };
    let standard = padded.replace('-', "+").replace('_', "/");

    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        standard.as_bytes(),
    )
    .ok()?;

    let json: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    json.get("exp")?.as_i64()
}

/// auth_method 规范化（导入路径用）
pub fn normalize_import_auth_method(
    auth_method: &str,
    _client_id: &str,
    _client_secret: &str,
    token_endpoint: &str,
) -> String {
    let am = auth_method.trim().to_lowercase();
    match am.as_str() {
        "external_idp" | "azuread" | "azure" | "entra" | "entra-id" | "microsoft" | "m365"
        | "office365" | "external" => "external_idp".to_string(),
        "social" | "google" | "github" => "social".to_string(),
        "idc" | "builderid" | "builder-id" | "iam" | "enterprise" => "idc".to_string(),
        "api_key" | "apikey" => "api_key".to_string(),
        _ => {
            // 推断：有 tokenEndpoint → external_idp
            if !token_endpoint.trim().is_empty() {
                return "external_idp".to_string();
            }
            // 默认 social
            if am.is_empty() {
                "social".to_string()
            } else {
                am
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_jwt_exp_valid() {
        // {"exp":1782990878} → Base64URL encoded
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            b"{\"exp\":1782990878}",
        );
        let token = format!("header.{payload}.signature");
        assert_eq!(parse_jwt_exp(&token), Some(1782990878));
    }

    #[test]
    fn test_parse_jwt_exp_malformed() {
        assert_eq!(parse_jwt_exp("not-a-jwt"), None);
        assert_eq!(parse_jwt_exp(""), None);
        assert_eq!(parse_jwt_exp("a.!!!invalid-base64.c"), None);
    }

    #[test]
    fn test_normalize_auth_method_truth_table() {
        let f = |am: &str, te: &str| normalize_import_auth_method(am, "", "", te);
        assert_eq!(f("external_idp", ""), "external_idp");
        assert_eq!(f("AzureAD", ""), "external_idp");
        assert_eq!(f("ENTRA", ""), "external_idp");
        assert_eq!(f("entra-id", ""), "external_idp");
        assert_eq!(f("microsoft", ""), "external_idp");
        assert_eq!(f("m365", ""), "external_idp");
        assert_eq!(f("office365", ""), "external_idp");
        assert_eq!(f("external", ""), "external_idp");
        assert_eq!(f("azure", ""), "external_idp");
        // tokenEndpoint 推断
        assert_eq!(f("", "https://login.microsoftonline.com/t/token"), "external_idp");
        // 常规路由
        assert_eq!(f("social", ""), "social");
        assert_eq!(f("idc", ""), "idc");
        assert_eq!(f("builder-id", ""), "idc");
        assert_eq!(f("", ""), "social");
    }

    #[test]
    fn test_validate_valid_endpoint() {
        assert!(validate_with_default_allowlist(
            "https://login.microsoftonline.com/tenant/oauth2/v2.0/token"
        )
        .is_ok());
        assert!(validate_with_default_allowlist(
            "https://login.microsoftonline.us/tenant/oauth2/v2.0/token"
        )
        .is_ok());
        assert!(validate_with_default_allowlist(
            "https://login.chinacloudapi.cn/tenant/oauth2/v2.0/token"
        )
        .is_ok());
    }

    #[test]
    fn test_reject_http() {
        let result = validate_with_default_allowlist(
            "http://login.microsoftonline.com/tenant/token",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("scheme must be https"));
    }

    #[test]
    fn test_reject_ip_literal() {
        assert!(validate_with_default_allowlist("https://192.168.1.1/token").is_err());
        assert!(validate_with_default_allowlist("https://127.0.0.1/token").is_err());
        assert!(validate_with_default_allowlist("https://[::1]/token").is_err());
    }

    #[test]
    fn test_reject_subdomain_spoof() {
        assert!(validate_with_default_allowlist(
            "https://login.microsoftonline.com.attacker.com/token"
        )
        .is_err());
        assert!(validate_with_default_allowlist(
            "https://evil-login.microsoftonline.com.evil.com/token"
        )
        .is_err());
    }

    #[test]
    fn test_reject_non_allowlisted_host() {
        assert!(
            validate_with_default_allowlist("https://accounts.google.com/token").is_err()
        );
        assert!(validate_with_default_allowlist("https://evil.com/token").is_err());
    }

    #[test]
    fn test_custom_allowlist() {
        let custom = &["custom-idp.example.com"];
        assert!(
            validate_external_idp_endpoint("https://custom-idp.example.com/token", custom)
                .is_ok()
        );
        assert!(validate_external_idp_endpoint(
            "https://sub.custom-idp.example.com/token",
            custom
        )
        .is_ok());
        assert!(validate_external_idp_endpoint(
            "https://login.microsoftonline.com/token",
            custom
        )
        .is_err());
    }
}
