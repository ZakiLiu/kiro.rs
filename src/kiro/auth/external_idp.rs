use anyhow::bail;
use chrono::{Duration, Utc};
use serde::Deserialize;
use url::Url;

use crate::http_client::{ProxyConfig, build_client};
use crate::model::config::TlsBackend;
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

// ── Browser SSO Flow (OIDC Authorization Code + PKCE) ──

#[derive(Debug, Deserialize)]
pub struct OidcDiscoveryConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
}

pub async fn oidc_discovery(
    issuer_url: &str,
    proxy: Option<&ProxyConfig>,
    tls_backend: TlsBackend,
) -> anyhow::Result<OidcDiscoveryConfig> {
    validate_with_default_allowlist(issuer_url)
        .map_err(|e| anyhow::anyhow!("OIDC discovery blocked: {e}"))?;

    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer_url.trim_end_matches('/')
    );

    let client = build_client(proxy, 30, tls_backend)?;
    let response = client.get(&discovery_url).send().await?;

    if !response.status().is_success() {
        bail!(
            "OIDC discovery failed: {} {}",
            response.status(),
            response.text().await.unwrap_or_default()
        );
    }

    let config: OidcDiscoveryConfig = response.json().await?;

    // SSRF 验证 discovery 返回的端点
    validate_with_default_allowlist(&config.authorization_endpoint)
        .map_err(|e| anyhow::anyhow!("discovery authorization_endpoint rejected: {e}"))?;
    validate_with_default_allowlist(&config.token_endpoint)
        .map_err(|e| anyhow::anyhow!("discovery token_endpoint rejected: {e}"))?;

    Ok(config)
}

/// 构建 Azure AD 授权 URL
pub fn build_authorization_url(
    authorization_endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    state: &str,
    scopes: &str,
) -> String {
    format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        authorization_endpoint,
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(scopes),
        urlencoding::encode(code_challenge),
        urlencoding::encode(state),
    )
}

/// 用授权码交换 token（Authorization Code Grant）
pub async fn exchange_code_for_token(
    token_endpoint: &str,
    client_id: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    proxy: Option<&ProxyConfig>,
    tls_backend: TlsBackend,
) -> anyhow::Result<ExternalIdpRefreshResponse> {
    validate_with_default_allowlist(token_endpoint)
        .map_err(|e| anyhow::anyhow!("token exchange blocked: {e}"))?;

    let client = build_client(proxy, 60, tls_backend)?;
    let form = [
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
    ];

    let response = client
        .post(token_endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .form(&form)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("external_idp token exchange failed: {} {}", status, body);
    }

    Ok(response.json().await?)
}

/// 默认 SSO scope（固定 Kiro scopes + offline_access）
pub fn default_sso_scopes(client_id: &str) -> String {
    format!(
        "api://{client_id}/codewhisperer:conversations api://{client_id}/codewhisperer:completions offline_access"
    )
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

    #[tokio::test]
    #[ignore] // requires network access to Microsoft token endpoint
    async fn test_live_refresh_external_idp() {
        let json = std::fs::read_to_string(
            "examples/CLIProxyAPI_licia.anguillara-sharevn.bond.json",
        )
        .unwrap();
        let cred: KiroCredentials = serde_json::from_str(&json).unwrap();

        assert_eq!(cred.auth_method.as_deref(), Some("external_idp"));
        assert!(cred.refresh_token.is_some());
        assert!(cred.token_endpoint.is_some());
        assert!(cred.client_id.is_some());

        let config = Config::default();
        let result = refresh_external_idp_token(&cred, &config, None).await;

        match result {
            Ok(new_cred) => {
                assert!(new_cred.access_token.is_some(), "should get new access_token");
                assert!(new_cred.expires_at.is_some(), "should get new expires_at");
                let new_token = new_cred.access_token.as_deref().unwrap();
                assert!(new_token.starts_with("eyJ"), "access_token should be a JWT");
                let exp = parse_jwt_exp(new_token);
                assert!(exp.is_some(), "new JWT should have exp");
                eprintln!("✅ Live refresh succeeded! New token exp: {:?}", exp);
                eprintln!("   New expires_at: {:?}", new_cred.expires_at);
                if new_cred.refresh_token != cred.refresh_token {
                    eprintln!("   ⚠️ Refresh token rotated!");
                }
            }
            Err(e) => {
                let err_str = e.to_string();
                if err_str.contains("invalid_grant") {
                    eprintln!("⚠️ Refresh token expired (invalid_grant) — need fresh login");
                } else {
                    panic!("❌ Live refresh failed: {}", e);
                }
            }
        }
    }

    #[test]
    fn test_deserialize_ide_format() {
        let json = std::fs::read_to_string("examples/kiro-auth-token.json").unwrap();
        let cred: KiroCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(cred.auth_method.as_deref(), Some("external_idp"));
        assert!(cred.access_token.is_some());
        assert!(cred.refresh_token.is_some());
        assert!(cred.token_endpoint.is_some());
        assert!(cred.issuer_url.is_some());
        assert!(cred.client_id.is_some());
        assert_eq!(cred.provider.as_deref(), Some("ExternalIdp"));
        assert!(cred.expires_at.is_some());
        assert!(
            cred.token_endpoint.as_deref().unwrap().contains("login.microsoftonline.com"),
            "tokenEndpoint should contain Microsoft host"
        );
    }

    #[test]
    fn test_deserialize_helper_format() {
        let json = std::fs::read_to_string(
            "examples/CLIProxyAPI_licia.anguillara-sharevn.bond.json",
        )
        .unwrap();
        let cred: KiroCredentials = serde_json::from_str(&json).unwrap();
        assert_eq!(cred.auth_method.as_deref(), Some("external_idp"));
        assert!(cred.access_token.is_some(), "access_token should be deserialized from snake_case");
        assert!(cred.refresh_token.is_some(), "refresh_token should be deserialized from snake_case");
        assert!(cred.token_endpoint.is_some(), "token_endpoint should be deserialized from snake_case");
        assert!(cred.issuer_url.is_some(), "issuer_url should be deserialized from snake_case");
        assert!(cred.client_id.is_some());
        assert!(cred.scopes.is_some(), "scopes should be present");
        assert!(cred.profile_arn.is_some(), "profile_arn should be present");
        assert!(cred.region.is_some());
        // expires_at 来自 "expired" alias
        assert!(cred.expires_at.is_some(), "expires_at should be deserialized from 'expired' alias");
    }

    #[test]
    fn test_parse_jwt_exp_from_real_token() {
        let json = std::fs::read_to_string("examples/kiro-auth-token.json").unwrap();
        let cred: KiroCredentials = serde_json::from_str(&json).unwrap();
        let exp = parse_jwt_exp(cred.access_token.as_deref().unwrap());
        assert!(exp.is_some(), "should extract exp from real Azure AD JWT");
        assert!(exp.unwrap() > 1700000000, "exp should be a valid Unix timestamp");
    }
}
