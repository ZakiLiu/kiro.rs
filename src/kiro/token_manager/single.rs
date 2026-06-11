//! 单凭据 Token 管理器

use crate::http_client::ProxyConfig;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::usage_limits::UsageLimitsResponse;
use crate::model::config::Config;
use anyhow::bail;
use chrono::{DateTime, Duration, Utc};

use super::refresh::refresh_token;
use super::types::get_usage_limits;

pub struct TokenManager {
    config: Config,
    credentials: KiroCredentials,
    proxy: Option<ProxyConfig>,
}

#[allow(dead_code)]
impl TokenManager {
    /// 创建新的 TokenManager 实例
    pub fn new(config: Config, credentials: KiroCredentials, proxy: Option<ProxyConfig>) -> Self {
        Self {
            config,
            credentials,
            proxy,
        }
    }

    /// 获取凭据的引用
    pub fn credentials(&self) -> &KiroCredentials {
        &self.credentials
    }

    /// 获取配置的引用
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// 确保获取有效的访问 Token
    ///
    /// 如果 Token 过期或即将过期，会自动刷新
    pub async fn ensure_valid_token(&mut self) -> anyhow::Result<String> {
        let token_missing_or_truncated = self
            .credentials
            .access_token
            .as_deref()
            .is_none_or(|t| t.trim().is_empty() || t.ends_with("...") || t.contains("..."));

        if token_missing_or_truncated
            || is_token_expired(&self.credentials)
            || is_token_expiring_soon(&self.credentials)
        {
            self.credentials =
                refresh_token(&self.credentials, &self.config, self.proxy.as_ref()).await?;

            // 刷新后再次检查 token 时间有效性
            if is_token_expired(&self.credentials) {
                anyhow::bail!("刷新后的 Token 仍然无效或已过期");
            }
        }

        self.credentials
            .access_token
            .clone()
            .ok_or_else(|| anyhow::anyhow!("没有可用的 accessToken"))
    }

    /// 获取使用额度信息
    ///
    /// 调用 getUsageLimits API 查询当前账户的使用额度
    pub async fn get_usage_limits(&mut self) -> anyhow::Result<UsageLimitsResponse> {
        let token = self.ensure_valid_token().await?;
        get_usage_limits(&self.credentials, &self.config, &token, self.proxy.as_ref()).await
    }
}

/// 检查 Token 是否在指定时间内过期
pub(crate) fn is_token_expiring_within(
    credentials: &KiroCredentials,
    minutes: i64,
) -> Option<bool> {
    credentials
        .expires_at
        .as_ref()
        .and_then(|expires_at| DateTime::parse_from_rfc3339(expires_at).ok())
        .map(|expires| expires <= Utc::now() + Duration::minutes(minutes))
}

/// 检查 Token 是否已过期（提前 5 分钟判断）
pub(crate) fn is_token_expired(credentials: &KiroCredentials) -> bool {
    if credentials.is_api_key_credential() {
        return false;
    }
    is_token_expiring_within(credentials, 5).unwrap_or(true)
}

/// 检查 Token 是否即将过期（10分钟内）
pub(crate) fn is_token_expiring_soon(credentials: &KiroCredentials) -> bool {
    if credentials.is_api_key_credential() {
        return false;
    }
    is_token_expiring_within(credentials, 10).unwrap_or(false)
}

/// 验证凭据的基本有效性
pub(crate) fn validate_credential_secret(credentials: &KiroCredentials) -> anyhow::Result<()> {
    if credentials.is_api_key_credential() {
        let api_key = credentials
            .kiro_api_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("缺少 kiroApiKey"))?;

        if api_key.trim().is_empty() {
            bail!("kiroApiKey 为空");
        }

        return Ok(());
    }

    validate_refresh_token(credentials)
}

/// 验证 refreshToken 的基本有效性
pub(crate) fn validate_refresh_token(credentials: &KiroCredentials) -> anyhow::Result<()> {
    let refresh_token = credentials
        .refresh_token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("缺少 refreshToken"))?;

    if refresh_token.is_empty() {
        bail!("refreshToken 为空");
    }

    if refresh_token.len() < 100 || refresh_token.ends_with("...") || refresh_token.contains("...")
    {
        bail!(
            "refreshToken 已被截断（长度: {} 字符）。\n\
             这通常是 Kiro IDE 为了防止凭证被第三方工具使用而故意截断的。",
            refresh_token.len()
        );
    }

    Ok(())
}
