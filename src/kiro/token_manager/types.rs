//! 共享类型与工具函数

use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::endpoint::{
    CLI_ENDPOINT_NAME, CliEndpoint, IDE_ENDPOINT_NAME, IDE_RUNTIME_ENDPOINT_NAME, IdeEndpoint,
    KiroEndpoint, RequestContext,
};
pub(crate) use crate::kiro::kiro_version::USAGE_API_KIRO_VERSION;
use crate::kiro::machine_id;
use crate::kiro::model::available_models::{ListAvailableModelsResponse, runtime_fallback_models};
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::model::usage_limits::UsageLimitsResponse;
use crate::model::config::Config;
use anyhow::bail;
use serde::Serialize;

/// 用量类 REST 接口（getUsageLimits / ListAvailableModels）固定使用的 Kiro IDE 版本。
///
/// 参考项目已验证：新版 IDE 版本在这些接口上会触发更严格的请求形态校验；
/// `ListAvailableModels` 继续使用运行时 `config.kiro_version` 时，Free 凭据可能返回
/// `400 Improperly formed request`。
/// 对 user_id 进行掩码处理，保护隐私
pub(super) fn mask_user_id(user_id: Option<&str>) -> String {
    match user_id {
        Some(id) => {
            let len = id.len();
            if len > 12 {
                format!("{}***{}", &id[..4], &id[len - 4..])
            } else {
                "***".to_string()
            }
        }
        None => "None".to_string(),
    }
}

fn usage_endpoint_for_credentials(
    credentials: &KiroCredentials,
    config: &Config,
) -> anyhow::Result<Box<dyn KiroEndpoint>> {
    match credentials.effective_endpoint_name(Some(&config.default_endpoint)) {
        IDE_ENDPOINT_NAME | IDE_RUNTIME_ENDPOINT_NAME => Ok(Box::new(IdeEndpoint::new())),
        CLI_ENDPOINT_NAME => Ok(Box::new(CliEndpoint::new())),
        name => bail!("未知 endpoint: {}", name),
    }
}

fn usage_request_parts_without_profile_arn(
    credentials: &KiroCredentials,
    config: &Config,
    token: &str,
    machine_id: &str,
) -> anyhow::Result<crate::kiro::endpoint::UsageRequestParts> {
    let endpoint = usage_endpoint_for_credentials(credentials, config)?;
    let ctx = RequestContext {
        credentials,
        token,
        machine_id,
        config,
    };
    let mut usage = endpoint.usage_request_parts(&ctx)?;

    let mut url = url::Url::parse(&usage.url)
        .map_err(|error| anyhow::anyhow!("解析用量查询 URL 失败: {}", error))?;
    let query_pairs: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| key != "profileArn")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    url.set_query(None);
    if !query_pairs.is_empty() {
        let mut query = url.query_pairs_mut();
        for (key, value) in query_pairs {
            query.append_pair(&key, &value);
        }
    }
    usage.url = url.to_string();

    Ok(usage)
}

fn available_models_url(host: &str) -> String {
    format!("https://{}/ListAvailableModels?origin=AI_EDITOR", host)
}

/// 获取使用额度信息
pub(crate) async fn get_usage_limits(
    credentials: &KiroCredentials,
    config: &Config,
    token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<UsageLimitsResponse> {
    tracing::debug!(
        endpoint = %credentials.effective_endpoint_name(Some(&config.default_endpoint)),
        "正在获取使用额度信息..."
    );

    let machine_id = machine_id::generate_from_credentials(credentials, config)
        .ok_or_else(|| anyhow::anyhow!("无法生成 machineId"))?;
    let usage = usage_request_parts_without_profile_arn(credentials, config, token, &machine_id)?;

    let client = build_client(proxy, 60, config.tls_backend)?;
    let mut request = client.get(&usage.url);
    for (name, value) in usage.headers {
        request = request.header(name, value);
    }

    let response = request.send().await?;

    let status = response.status();
    if !status.is_success() {
        let body_text = response.text().await.unwrap_or_default();
        let error_msg = match status.as_u16() {
            401 => "认证失败，Token 无效或已过期",
            403 => "权限不足，无法获取使用额度",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS 服务暂时不可用",
            _ => "获取使用额度失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    let body_text = response.text().await?;
    let data: UsageLimitsResponse = serde_json::from_str(&body_text).map_err(|e| {
        tracing::error!(
            "getUsageLimits JSON 解析失败: {}，原始响应: {}",
            e,
            body_text
        );
        anyhow::anyhow!("JSON 解析失败: {}", e)
    })?;
    Ok(data)
}

/// 获取凭据可用模型列表
pub(crate) async fn get_available_models(
    credentials: &KiroCredentials,
    config: &Config,
    token: &str,
    proxy: Option<&ProxyConfig>,
) -> anyhow::Result<ListAvailableModelsResponse> {
    if credentials.effective_endpoint_name(Some(&config.default_endpoint))
        == IDE_RUNTIME_ENDPOINT_NAME
    {
        tracing::debug!(
            "runtime.kiro.dev endpoint 不支持 ListAvailableModels，使用静态 fallback 模型列表"
        );
        return Ok(runtime_fallback_models());
    }

    let sso_region = credentials.effective_auth_region(config);
    let candidates = rest_api_region_candidates(sso_region);
    let machine_id = machine_id::generate_from_credentials(credentials, config)
        .ok_or_else(|| anyhow::anyhow!("无法生成 machineId"))?;
    let kiro_version = USAGE_API_KIRO_VERSION;
    let os_name = &config.system_version;
    let node_version = &config.node_version;

    let user_agent = format!(
        "aws-sdk-js/1.0.0 ua/2.1 os/{} lang/js md/nodejs#{} api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{}-{}",
        os_name, node_version, kiro_version, machine_id
    );
    let amz_user_agent = format!("aws-sdk-js/1.0.0 KiroIDE-{}-{}", kiro_version, machine_id);

    let client = build_client(proxy, 60, config.tls_backend)?;

    let mut last_error: Option<String> = None;
    for (idx, region) in candidates.iter().enumerate() {
        let host = format!("q.{}.amazonaws.com", region);
        let url = available_models_url(&host);

        let mut request = client
            .get(&url)
            .header("x-amz-user-agent", &amz_user_agent)
            .header("user-agent", &user_agent)
            .header("host", &host)
            .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=1")
            .header("Authorization", format!("Bearer {}", token))
            .header("Connection", "close");

        if credentials.is_api_key_credential() {
            request = request.header("tokentype", "API_KEY");
        }

        let response = request.send().await?;
        let status = response.status();

        if status.is_success() {
            return response
                .json::<ListAvailableModelsResponse>()
                .await
                .map_err(|e| anyhow::anyhow!("解析可用模型响应失败: {}", e));
        }

        let body_text = response.text().await.unwrap_or_default();
        if status.as_u16() == 403 && idx + 1 < candidates.len() {
            last_error = Some(format!("{} {}", status, body_text));
            continue;
        }

        let error_msg = match status.as_u16() {
            401 => "认证失败，Token 无效或已过期",
            403 => "权限不足，无法获取可用模型",
            429 => "请求过于频繁，已被限流",
            500..=599 => "服务器错误，AWS 服务暂时不可用",
            _ => "获取可用模型失败",
        };
        bail!("{}: {} {}", error_msg, status, body_text);
    }

    // 所有区域 403：该凭据无 ListAvailableModels 权限，返回空列表而非报错
    if last_error
        .as_deref()
        .is_some_and(|e| e.starts_with("403"))
    {
        tracing::info!(
            "凭据无 ListAvailableModels 权限（所有区域 403），返回空模型列表"
        );
        return Ok(ListAvailableModelsResponse::default());
    }

    bail!(
        "获取可用模型失败: {}",
        last_error.unwrap_or_else(|| "无可用端点".to_string())
    )
}

fn rest_api_region_candidates(sso_region: &str) -> Vec<&str> {
    if sso_region.starts_with("eu-") {
        vec!["eu-central-1", "us-east-1"]
    } else {
        vec!["us-east-1", "eu-central-1"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_usage_api_kiro_version_is_pinned_for_rest_usage_apis() {
        assert_eq!(USAGE_API_KIRO_VERSION, "0.9.2");
    }

    #[test]
    fn test_usage_rest_requests_omit_profile_arn() {
        let config = Config::default();
        let credentials = KiroCredentials {
            profile_arn: Some(
                "arn:aws:codewhisperer:us-east-1:123456789012:profile/REAL123".to_string(),
            ),
            ..KiroCredentials::default()
        };

        let usage =
            usage_request_parts_without_profile_arn(&credentials, &config, "token", "machine123")
                .unwrap();
        assert!(usage.url.contains("getUsageLimits"));
        assert!(!usage.url.contains("profileArn"));

        assert_eq!(
            available_models_url("q.us-east-1.amazonaws.com"),
            "https://q.us-east-1.amazonaws.com/ListAvailableModels?origin=AI_EDITOR"
        );
    }

    #[test]
    fn test_runtime_endpoint_usage_uses_legacy_ide_rest_endpoint() {
        let mut config = Config::default();
        config.default_endpoint = IDE_RUNTIME_ENDPOINT_NAME.to_string();
        config.region = "us-east-1".to_string();
        config.api_region = None;
        config.system_version = "win32#10.0.22631".to_string();
        config.node_version = "22.22.0".to_string();

        let credentials = KiroCredentials::default();
        let endpoint = usage_endpoint_for_credentials(&credentials, &config).unwrap();
        assert_eq!(endpoint.name(), IDE_ENDPOINT_NAME);

        let ctx = RequestContext {
            credentials: &credentials,
            token: "token",
            machine_id: "machine123",
            config: &config,
        };
        let usage = endpoint.usage_request_parts(&ctx).unwrap();

        assert!(
            usage
                .url
                .starts_with("https://q.us-east-1.amazonaws.com/getUsageLimits")
        );
        assert!(!usage.url.contains("runtime.us-east-1.kiro.dev"));
        assert!(usage.url.contains("resourceType=AGENTIC_REQUEST"));

        let host = usage
            .headers
            .iter()
            .find(|(name, _)| *name == "host")
            .map(|(_, value)| value.as_str())
            .unwrap();
        assert_eq!(host, "q.us-east-1.amazonaws.com");
    }

    #[test]
    fn test_credential_runtime_endpoint_usage_uses_legacy_ide_rest_endpoint() {
        let config = Config::default();
        let credentials = KiroCredentials {
            endpoint: Some(IDE_RUNTIME_ENDPOINT_NAME.to_string()),
            ..KiroCredentials::default()
        };

        let endpoint = usage_endpoint_for_credentials(&credentials, &config).unwrap();
        assert_eq!(endpoint.name(), IDE_ENDPOINT_NAME);
    }

    #[tokio::test]
    async fn test_runtime_endpoint_get_available_models_uses_static_fallback() {
        let mut config = Config::default();
        config.default_endpoint = IDE_RUNTIME_ENDPOINT_NAME.to_string();

        let credentials = KiroCredentials::default();
        let models = get_available_models(&credentials, &config, "not_a_real_token", None)
            .await
            .unwrap();

        let ids: Vec<&str> = models
            .models
            .iter()
            .map(|model| model.model_id.as_str())
            .collect();
        assert!(ids.contains(&"claude-sonnet-4.6"));
        assert!(ids.contains(&"claude-opus-4.8"));
        assert!(ids.contains(&"claude-haiku-4.5"));
        assert!(ids.contains(&"deepseek-3.2"));
        assert!(ids.contains(&"glm-5"));
        assert!(ids.contains(&"minimax-m2.1"));
        assert!(ids.contains(&"minimax-m2.5"));
        assert!(ids.contains(&"qwen3-coder-next"));
    }
}

// ============================================================================
// 多凭据 Token 管理器
// ============================================================================

/// 凭据禁用原因
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisableReason {
    /// 连续失败次数过多
    FailureLimit,
    /// Token 刷新连续失败次数过多
    RefreshFailureLimit,
    /// 认证失败（如 invalid_grant）
    AuthenticationFailed,
    /// 历史遗留的账户暂停禁用原因；新逻辑只用于迁移旧配置，运行时改用 cooldown
    AccountSuspended,
    /// 余额不足
    #[allow(dead_code)]
    InsufficientBalance,
    /// 模型临时不可用（全局禁用，已废弃——改为 per-credential cooldown）
    #[allow(dead_code)]
    ModelUnavailable,
    /// 手动禁用
    Manual,
    /// 额度已用尽（如 MONTHLY_REQUEST_COUNT）
    QuotaExceeded,
}

/// 凭据条目快照（用于 Admin API 读取）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialEntrySnapshot {
    /// 凭据唯一 ID
    pub id: u64,
    /// 优先级
    pub priority: u32,
    /// 是否被禁用
    pub disabled: bool,
    /// 禁用原因
    pub disable_reason: Option<DisableReason>,
    /// 禁用时间
    pub disabled_at: Option<String>,
    /// 恢复尝试次数
    pub recovery_attempts: u32,
    /// 连续失败次数
    pub failure_count: u32,
    /// Token 刷新连续失败次数
    pub refresh_failure_count: u32,
    /// 认证方式
    pub auth_method: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// Token 过期时间
    pub expires_at: Option<String>,
    /// refreshToken 的 SHA-256 哈希（用于前端重复检测）
    pub refresh_token_hash: Option<String>,
    /// 用户邮箱（用于前端显示）
    pub email: Option<String>,
    /// 已持久化的订阅等级（页面刷新后可直接展示）
    pub subscription_title: Option<String>,
    /// API 调用成功次数
    pub success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    pub last_used_at: Option<String>,
    /// 凭据级 Region（用于 Token 刷新）
    pub region: Option<String>,
    /// 凭据级 API Region（单独覆盖 API 请求）
    pub api_region: Option<String>,
    /// 最终生效的 endpoint 名称
    pub endpoint: Option<String>,
    /// 账号所属分组
    pub groups: Vec<String>,
    /// 账号来源渠道
    pub source_channel: Option<String>,
    /// 是否可立即承接请求（未禁用、未冷却、未触发本地速率限制）
    pub ready: bool,
    /// 冷却原因（人类可读描述）
    pub cooldown_reason: Option<String>,
    /// 冷却剩余秒数（向上取整）
    pub cooldown_remaining_secs: Option<u64>,
    /// 是否被本地速率限制挡住
    pub rate_limited: bool,
    /// 本地速率限制剩余秒数（向上取整）
    pub rate_limit_remaining_secs: Option<u64>,
    /// 累计收到 suspended 信号的次数
    pub suspended_count: u32,
    /// 禁用时的错误详情（上游返回的原始消息）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disable_message: Option<String>,
}

/// 凭据管理器状态快照
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerSnapshot {
    /// 凭据条目列表
    pub entries: Vec<CredentialEntrySnapshot>,
    /// 总凭据数量
    pub total: usize,
    /// 可用凭据数量
    ///
    /// 兼容旧 Admin UI：这里仍表示“未禁用凭据数量”，不代表可立即承接请求。
    pub available: usize,
    /// 可立即承接请求的凭据数量
    pub ready: usize,
    /// 当前处于 cooldown 的未禁用凭据数量
    pub cooling: usize,
}

/// 缓存余额信息（用于 Admin API）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedBalanceInfo {
    /// 凭据 ID
    pub id: u64,
    /// 缓存的剩余额度
    pub remaining: f64,
    /// 缓存时间（Unix 毫秒时间戳）
    pub cached_at: u64,
    /// 缓存存活时间（秒）
    pub ttl_secs: u64,
}
