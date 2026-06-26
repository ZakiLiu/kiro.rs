//! Admin API 业务逻辑服务

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Duration, Timelike, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::anthropic::PromptCacheRuntime;
use crate::common::utf8::floor_char_boundary;
use crate::http_client::ProxyConfig;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::provider::KiroProvider;
use crate::kiro::token_manager::{CachedBalanceInfo, MultiTokenManager};
use crate::metrics::{MetricEventType, MetricsCollector};
use crate::model::config::{CompressionConfig, Config, is_supported_prompt_cache_ttl_seconds};
use parking_lot::RwLock;

use super::error::AdminServiceError;
use super::proxy_pool::ProxyPoolManager;
use super::types::{
    AddCredentialRequest, AddCredentialResponse, BalanceResponse, CachedBalanceItem,
    CachedBalancesResponse, CredentialMetrics, CredentialStatusItem, CredentialsStatusResponse,
    ImportAction, ImportItemResult, ImportSummary, ImportTokenJsonRequest, ImportTokenJsonResponse,
    MetricsSummaryResponse, ModelMetrics, ProxyConfigResponse, ProxyPoolEntry, TokenJsonItem,
    UpdateProxyConfigRequest,
};
use super::types::{
    CheckRateLimitRequest, GitHubRateLimitInfo, ImageUpdateResponse, PollIdcLoginResponse,
    SetUpdateConfigRequest, StartIdcLoginResponse, StartSocialLoginResponse, UpdateCheckInfo,
    UpdateConfigResponse,
};
use crate::kiro::auth::idc::{self, BUILDER_ID_START_URL};
use crate::kiro::auth::social;

/// 余额缓存过期时间（秒），5 分钟
const BALANCE_CACHE_TTL_SECS: i64 = 300;

/// 在线检查更新结果缓存时间（秒），30 分钟
const UPDATE_CHECK_TTL_SECS: i64 = 1800;

const BUILD_TYPE: &str = "binary";

const GITHUB_RELEASES_REPO: &str = "ZakiLiu/kiro.rs";

#[derive(Debug, Clone)]
struct CachedUpdateCheck {
    cached_at: DateTime<Utc>,
    info: UpdateCheckInfo,
}

#[derive(Debug, Clone)]
struct RuntimeUpdateConfig {
    previous_version: Option<String>,
    last_applied_at: Option<String>,
    github_token: Option<String>,
    auto_apply: bool,
    auto_apply_time: String,
}

impl RuntimeUpdateConfig {
    fn from_config(config: &Config) -> Self {
        Self {
            previous_version: config.update_previous_version.clone(),
            last_applied_at: config.update_last_applied_at.clone(),
            github_token: config.github_token.clone(),
            auto_apply: config.update_auto_apply,
            auto_apply_time: config.update_auto_apply_time.clone(),
        }
    }

    fn response(&self) -> UpdateConfigResponse {
        UpdateConfigResponse {
            previous_version: self.previous_version.clone(),
            last_applied_at: self.last_applied_at.clone(),
            github_token_set: self
                .github_token
                .as_deref()
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false),
            auto_apply: self.auto_apply,
            auto_apply_time: self.auto_apply_time.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    #[serde(default)]
    name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    published_at: String,
    #[serde(default)]
    tag_name: String,
}

fn compare_semver(current: &str, latest: &str) -> std::cmp::Ordering {
    parse_semver_core(current).cmp(&parse_semver_core(latest))
}

fn parse_semver_core(value: &str) -> [u32; 3] {
    let core = value
        .trim_start_matches('v')
        .split(['-', '+'])
        .next()
        .unwrap_or("");
    let mut out = [0u32; 3];
    for (i, part) in core.splitn(3, '.').enumerate() {
        if i >= 3 {
            break;
        }
        out[i] = part.parse::<u32>().unwrap_or(0);
    }
    out
}

fn staged_binary_path(exe: &std::path::Path, version: &str) -> std::path::PathBuf {
    let mut s = exe.as_os_str().to_os_string();
    s.push(format!(
        ".staged-{}",
        version.trim().trim_start_matches('v')
    ));
    std::path::PathBuf::from(s)
}

fn cleanup_other_staged(exe: &std::path::Path, keep_version: &str) {
    let dir = match exe.parent() {
        Some(d) => d,
        None => return,
    };
    let exe_name = match exe.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return,
    };
    let keep = format!(
        "{}.staged-{}",
        exe_name,
        keep_version.trim().trim_start_matches('v')
    );
    let keep_metadata = format!("{}.metadata.json", keep);
    let prefix = format!("{}.staged-", exe_name);
    let entries = match std::fs::read_dir(dir) {
        Ok(it) => it,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if name.starts_with(&prefix) && name != keep && name != keep_metadata {
            let path = entry.path();
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(super::binary_update::staged_metadata_path(&path));
        }
    }
}

fn parse_auto_apply_time(value: &str) -> Result<(u32, u32), AdminServiceError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(AdminServiceError::InvalidRequest(
            "自动更新时间不能为空".to_string(),
        ));
    }
    let mut parts = trimmed.splitn(2, ':');
    let hour_str = parts.next().unwrap_or("");
    let minute_str = parts.next().unwrap_or("");
    let hour: u32 = hour_str.parse().map_err(|_| {
        AdminServiceError::InvalidRequest(format!("自动更新时间格式无效：{}（应为 HH:MM）", value))
    })?;
    let minute: u32 = minute_str.parse().map_err(|_| {
        AdminServiceError::InvalidRequest(format!("自动更新时间格式无效：{}（应为 HH:MM）", value))
    })?;
    if hour > 23 || minute > 59 {
        return Err(AdminServiceError::InvalidRequest(format!(
            "自动更新时间超出范围：{}（HH 0-23，MM 0-59）",
            value
        )));
    }
    Ok((hour, minute))
}

fn normalize_auto_apply_time(value: &str) -> Result<String, AdminServiceError> {
    let (h, m) = parse_auto_apply_time(value)?;
    Ok(format!("{:02}:{:02}", h, m))
}

/// 缓存的余额条目（含时间戳）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedBalance {
    /// 缓存时间（Unix 秒）
    cached_at: f64,
    /// 缓存的余额数据
    data: BalanceResponse,
}

/// Social 登录会话
struct SocialAuthSession {
    auth_endpoint: String,
    state: String,
    code_verifier: String,
    redirect_uri: String,
    expires_at: chrono::DateTime<Utc>,
    callback_rx: tokio::sync::Mutex<tokio::sync::oneshot::Receiver<social::OAuthCallbackData>>,
    cred_template: KiroCredentials,
    proxy: Option<crate::http_client::ProxyConfig>,
    _server_handle: Option<social::ServerHandle>,
    remote_callback_tx:
        Option<Mutex<Option<tokio::sync::oneshot::Sender<social::OAuthCallbackData>>>>,
    relogin_target_id: Option<u64>,
}

/// 远程 Social OAuth 公网回调投递结果
pub enum RemoteCallbackOutcome {
    Delivered,
    AlreadyCompleted,
    Expired,
    NotFound,
}

/// IdC 设备授权登录会话
#[allow(dead_code)]
struct IdcAuthSession {
    region: String,
    client_id: String,
    client_secret: String,
    device_code: String,
    expires_at: chrono::DateTime<Utc>,
    poll_interval: i64,
    cred_template: KiroCredentials,
    proxy: Option<crate::http_client::ProxyConfig>,
    relogin_target_id: Option<u64>,
}

/// Admin 服务
///
/// 封装所有 Admin API 的业务逻辑
pub struct AdminService {
    token_manager: Arc<MultiTokenManager>,
    kiro_provider: Option<Arc<KiroProvider>>,
    config: Arc<RwLock<Config>>,
    compression_config: Arc<RwLock<CompressionConfig>>,
    prompt_cache_runtime: Arc<RwLock<PromptCacheRuntime>>,
    metrics: Option<Arc<MetricsCollector>>,
    balance_cache: Mutex<HashMap<u64, CachedBalance>>,
    cache_path: Option<PathBuf>,
    known_endpoints: HashSet<String>,
    proxy_pool: ProxyPoolManager,
    social_sessions: Arc<Mutex<HashMap<String, SocialAuthSession>>>,
    idc_sessions: Arc<Mutex<HashMap<String, IdcAuthSession>>>,
    update_config: Mutex<RuntimeUpdateConfig>,
    update_check_cache: Mutex<Option<CachedUpdateCheck>>,
}

impl AdminService {
    pub fn new(
        token_manager: Arc<MultiTokenManager>,
        kiro_provider: Option<Arc<KiroProvider>>,
        config: Arc<RwLock<Config>>,
        compression_config: Arc<RwLock<CompressionConfig>>,
        prompt_cache_runtime: Arc<RwLock<PromptCacheRuntime>>,
        metrics: Option<Arc<MetricsCollector>>,
        known_endpoints: impl IntoIterator<Item = String>,
    ) -> Self {
        let cache_path = token_manager
            .cache_dir()
            .map(|d| d.join("kiro_balance_cache.json"));

        let balance_cache = Self::load_balance_cache_from(&cache_path);

        for (id, cached) in &balance_cache {
            token_manager.restore_balance_cache(*id, cached.data.remaining, cached.cached_at);
        }

        let proxy_pool_path = token_manager.cache_dir().map(|d| d.join("proxy_pool.json"));
        let tls_backend = config.read().tls_backend;
        let proxy_pool = ProxyPoolManager::new(proxy_pool_path, tls_backend);

        let update_config = RuntimeUpdateConfig::from_config(&config.read());

        Self {
            token_manager,
            kiro_provider,
            config,
            compression_config,
            prompt_cache_runtime,
            metrics,
            balance_cache: Mutex::new(balance_cache),
            cache_path,
            known_endpoints: known_endpoints.into_iter().collect(),
            proxy_pool,
            social_sessions: Arc::new(Mutex::new(HashMap::new())),
            idc_sessions: Arc::new(Mutex::new(HashMap::new())),
            update_config: Mutex::new(update_config),
            update_check_cache: Mutex::new(None),
        }
    }

    /// 导出凭据为 JSON 格式
    pub fn export_credentials(
        &self,
        id_filter: Option<&HashSet<u64>>,
    ) -> super::types::CredentialsExportResponse {
        let mut credentials = self.token_manager.clone_all_credentials();
        if let Some(filter) = id_filter {
            credentials.retain(|c| c.id.map(|id| filter.contains(&id)).unwrap_or(false));
        }
        credentials.sort_by_key(|c| c.priority);

        let accounts = credentials
            .into_iter()
            .filter_map(credential_to_export_account)
            .collect();

        super::types::CredentialsExportResponse {
            version: "1.8.3".to_string(),
            exported_at: Utc::now().timestamp_millis(),
            accounts,
            groups: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// 一键禁用所有超额凭据
    pub fn disable_quota_exceeded(&self) -> super::types::QuotaExceededResult {
        let snapshot = self.token_manager.snapshot();
        let cache_snapshot: HashMap<u64, CachedBalance> = {
            let cache = self.balance_cache.lock();
            cache.clone()
        };
        let now_ts = Utc::now().timestamp() as f64;

        let mut disabled_ids: Vec<u64> = Vec::new();
        let mut skipped_ids: Vec<u64> = Vec::new();

        for entry in snapshot.entries.iter() {
            if entry.disabled {
                continue;
            }
            let cached = match cache_snapshot.get(&entry.id) {
                Some(c) if (now_ts - c.cached_at) < BALANCE_CACHE_TTL_SECS as f64 => c,
                _ => continue,
            };
            if cached.data.remaining > 0.0 {
                continue;
            }
            match self.token_manager.disable_quota_exceeded(entry.id) {
                Ok(()) => disabled_ids.push(entry.id),
                Err(e) => {
                    tracing::warn!("一键超额：禁用凭据 #{} 失败: {}", entry.id, e);
                    skipped_ids.push(entry.id);
                }
            }
        }

        super::types::QuotaExceededResult {
            disabled_ids,
            skipped_ids,
        }
    }

    /// 设置凭据超额开关
    pub async fn set_overage(&self, id: u64, enabled: bool) -> Result<(), AdminServiceError> {
        let status = if enabled { "ENABLED" } else { "DISABLED" };
        self.token_manager
            .set_user_preference_for(id, status)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        {
            let mut cache = self.balance_cache.lock();
            cache.remove(&id);
        }
        self.save_balance_cache();
        Ok(())
    }

    /// 一键开启所有凭据超额
    pub async fn enable_overage_for_all_capable(&self) -> super::types::EnableOverageAllResult {
        let snapshot = self.token_manager.snapshot();
        let mut targets: Vec<u64> = Vec::new();
        let mut skipped: Vec<u64> = Vec::new();

        for entry in snapshot.entries.iter() {
            if entry.disabled {
                skipped.push(entry.id);
            } else {
                targets.push(entry.id);
            }
        }

        let mut enabled_ids: Vec<u64> = Vec::new();
        let mut failed_ids: Vec<u64> = Vec::new();
        let mut failure_messages: Vec<String> = Vec::new();

        for id in targets {
            match self
                .token_manager
                .set_user_preference_for(id, "ENABLED")
                .await
            {
                Ok(()) => {
                    enabled_ids.push(id);
                    let mut cache = self.balance_cache.lock();
                    cache.remove(&id);
                }
                Err(e) => {
                    tracing::warn!("一键开启超额：凭据 #{} 失败: {}", id, e);
                    failed_ids.push(id);
                    failure_messages.push(e.to_string());
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }

        if !enabled_ids.is_empty() {
            self.save_balance_cache();
        }

        super::types::EnableOverageAllResult {
            enabled_ids,
            skipped_ids: skipped,
            failed_ids,
            failure_messages,
        }
    }

    /// 获取所有凭据状态
    pub fn get_all_credentials(&self) -> CredentialsStatusResponse {
        let snapshot = self.token_manager.snapshot();

        let default_endpoint = self.config.read().default_endpoint.clone();
        let mut credentials: Vec<CredentialStatusItem> = snapshot
            .entries
            .into_iter()
            .map(|entry| {
                let endpoint = entry.endpoint;
                let effective_endpoint = endpoint.clone().unwrap_or(default_endpoint.clone());
                CredentialStatusItem {
                    id: entry.id,
                    priority: entry.priority,
                    disabled: entry.disabled,
                    is_current: false,
                    failure_count: entry.failure_count,
                    refresh_failure_count: entry.refresh_failure_count,
                    disabled_reason: entry.disable_reason.map(|reason| format!("{:?}", reason)),
                    ready: entry.ready,
                    cooldown_reason: entry.cooldown_reason,
                    cooldown_remaining_secs: entry.cooldown_remaining_secs,
                    rate_limited: entry.rate_limited,
                    rate_limit_remaining_secs: entry.rate_limit_remaining_secs,
                    expires_at: entry.expires_at,
                    auth_method: entry.auth_method,
                    has_profile_arn: entry.has_profile_arn,
                    refresh_token_hash: entry.refresh_token_hash,
                    email: entry.email,
                    subscription_title: entry.subscription_title,
                    success_count: entry.success_count,
                    last_used_at: entry.last_used_at.clone(),
                    region: entry.region,
                    api_region: entry.api_region,
                    endpoint,
                    effective_endpoint,
                    groups: entry.groups,
                    source_channel: entry.source_channel,
                }
            })
            .collect();

        // 按优先级排序（数字越小优先级越高）
        credentials.sort_by_key(|c| c.priority);

        // 当前活跃 = 最近使用的非禁用凭据
        let current_id = credentials
            .iter()
            .filter(|c| !c.disabled && c.last_used_at.is_some())
            .max_by(|a, b| a.last_used_at.cmp(&b.last_used_at))
            .map(|c| c.id)
            .or_else(|| credentials.iter().find(|c| !c.disabled).map(|c| c.id))
            .unwrap_or(0);

        // 标记当前活跃凭据
        for c in &mut credentials {
            if c.id == current_id {
                c.is_current = true;
            }
        }

        CredentialsStatusResponse {
            total: snapshot.total,
            available: snapshot.available,
            ready: snapshot.ready,
            cooling: snapshot.cooling,
            current_id,
            credentials,
        }
    }

    /// 设置凭据禁用状态
    pub fn set_disabled(&self, id: u64, disabled: bool) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_disabled(id, disabled)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 设置凭据优先级
    pub fn set_priority(&self, id: u64, priority: u32) -> Result<(), AdminServiceError> {
        self.token_manager
            .set_priority(id, priority)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 设置凭据 Region
    pub fn set_region(
        &self,
        id: u64,
        region: Option<String>,
        api_region: Option<String>,
    ) -> Result<(), AdminServiceError> {
        // trim 后空字符串转 None
        let region = region
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let api_region = api_region
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        self.token_manager
            .set_region(id, region, api_region)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 设置凭据 endpoint
    pub fn set_endpoint(&self, id: u64, endpoint: Option<String>) -> Result<(), AdminServiceError> {
        let endpoint = endpoint
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if let Some(name) = endpoint.as_deref()
            && !self.known_endpoints.contains(name)
        {
            let mut known: Vec<&str> = self.known_endpoints.iter().map(|s| s.as_str()).collect();
            known.sort_unstable();
            return Err(AdminServiceError::InvalidCredential(format!(
                "endpoint 必须是已注册值，已注册: {:?}，收到: {}",
                known, name
            )));
        }

        self.token_manager
            .set_endpoint(id, endpoint)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 重置失败计数并重新启用
    pub fn reset_and_enable(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .reset_and_enable(id)
            .map_err(|e| self.classify_error(e, id))
    }

    /// 强制刷新指定凭据 Token
    pub async fn force_refresh_token(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .force_refresh_token_for(id)
            .await
            .map_err(|e| self.classify_error(e, id))
    }

    /// 获取凭据可用模型列表
    pub async fn get_available_models(
        &self,
        id: u64,
    ) -> Result<super::types::AvailableModelsResponse, AdminServiceError> {
        let resp = self
            .token_manager
            .get_available_models_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        let models = resp
            .models
            .into_iter()
            .map(|m| super::types::AvailableModelItem {
                model_id: m.model_id,
                model_name: m.model_name,
                description: m.description,
                max_input_tokens: m.token_limits.and_then(|t| t.max_input_tokens),
            })
            .collect();

        Ok(super::types::AvailableModelsResponse { id, models })
    }

    /// 获取凭据余额（带缓存）
    pub async fn get_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        // 先查缓存
        {
            let cache = self.balance_cache.lock();
            if let Some(cached) = cache.get(&id) {
                let now = Utc::now().timestamp() as f64;
                if (now - cached.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    tracing::debug!("凭据 #{} 余额命中缓存", id);
                    return Ok(cached.data.clone());
                }
            }
        }

        // 缓存未命中或已过期，从上游获取
        let balance = self.fetch_balance(id).await?;

        // 更新缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.insert(
                id,
                CachedBalance {
                    cached_at: Utc::now().timestamp() as f64,
                    data: balance.clone(),
                },
            );
        }
        self.save_balance_cache();

        Ok(balance)
    }

    /// 从上游获取余额（无缓存）
    async fn fetch_balance(&self, id: u64) -> Result<BalanceResponse, AdminServiceError> {
        let usage = self
            .token_manager
            .get_usage_limits_for(id)
            .await
            .map_err(|e| self.classify_balance_error(e, id))?;

        let current_usage = usage.current_usage();
        let usage_limit = usage.usage_limit();
        let remaining = (usage_limit - current_usage).max(0.0);
        let usage_percentage = if usage_limit > 0.0 {
            (current_usage / usage_limit * 100.0).min(100.0)
        } else {
            0.0
        };

        // 更新缓存，使列表页面能显示最新余额
        self.token_manager.update_balance_cache(id, remaining);

        // KIRO PRO 超额检查
        self.token_manager
            .check_pro_overuse_disable(id, usage.subscription_title(), current_usage);
        // 自动按订阅等级归类分组
        self.token_manager
            .auto_assign_subscription_group(id, usage.subscription_title());

        Ok(BalanceResponse {
            id,
            subscription_title: usage.subscription_title().map(|s| s.to_string()),
            current_usage,
            usage_limit,
            remaining,
            usage_percentage,
            next_reset_at: usage.next_date_reset,
            overage_enabled: usage.overage_enabled(),
            overage_capable: usage.overage_capable(),
            overage_capability_raw: usage.overage_capability_raw().map(|s| s.to_string()),
        })
    }

    /// 获取所有凭据的缓存余额
    pub fn get_cached_balances(&self) -> CachedBalancesResponse {
        // 从 token_manager 获取运行时缓存（含 TTL 信息）
        let runtime_balances: HashMap<u64, CachedBalanceInfo> = self
            .token_manager
            .get_all_cached_balances()
            .into_iter()
            .map(|info| (info.id, info))
            .collect();

        // 从 AdminService 磁盘缓存获取完整余额信息
        let disk_cache = self.balance_cache.lock();

        let balances = runtime_balances
            .into_iter()
            .map(|(id, info)| {
                // 优先从磁盘缓存获取完整快照（保证字段一致性）
                if let Some(cached) = disk_cache.get(&id) {
                    CachedBalanceItem {
                        id,
                        remaining: cached.data.remaining,
                        usage_limit: cached.data.usage_limit,
                        usage_percentage: cached.data.usage_percentage,
                        subscription_title: cached.data.subscription_title.clone(),
                        cached_at: info.cached_at,
                        ttl_secs: info.ttl_secs,
                    }
                } else {
                    CachedBalanceItem {
                        id,
                        remaining: info.remaining,
                        usage_limit: 0.0,
                        usage_percentage: 0.0,
                        subscription_title: None,
                        cached_at: info.cached_at,
                        ttl_secs: info.ttl_secs,
                    }
                }
            })
            .collect();

        CachedBalancesResponse { balances }
    }

    /// 添加新凭据
    pub async fn add_credential(
        &self,
        req: AddCredentialRequest,
    ) -> Result<AddCredentialResponse, AdminServiceError> {
        // 构建凭据对象
        let email = req.email.clone();
        let effective_auth_method = if req
            .kiro_api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
        {
            "api_key".to_string()
        } else {
            req.auth_method.clone()
        };
        let endpoint = req
            .endpoint
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(name) = endpoint.as_deref()
            && !self.known_endpoints.contains(name)
        {
            let mut known: Vec<&str> = self.known_endpoints.iter().map(|s| s.as_str()).collect();
            known.sort_unstable();
            return Err(AdminServiceError::InvalidCredential(format!(
                "endpoint 必须是已注册值，已注册: {:?}，收到: {}",
                known, name
            )));
        }
        let new_cred = KiroCredentials {
            id: None,
            access_token: None,
            refresh_token: req.refresh_token,
            kiro_api_key: req.kiro_api_key,
            profile_arn: None,
            expires_at: None,
            auth_method: Some(effective_auth_method),
            client_id: req.client_id,
            client_secret: req.client_secret,
            priority: req.priority,
            region: req.region,
            api_region: req.api_region,
            machine_id: req.machine_id,
            endpoint,
            email: req.email,
            subscription_title: None,
            proxy_url: req.proxy_url,
            proxy_username: req.proxy_username,
            proxy_password: req.proxy_password,
            groups: req.groups,
            source_channel: req.source_channel,
            disabled: false,
            disable_reason: None,
            runtime_only: false,
        };

        // 调用 token_manager 添加凭据
        let credential_id = self
            .token_manager
            .add_credential(new_cred)
            .await
            .map_err(|e| self.classify_add_error(e))?;

        if let Err(e) = self.token_manager.get_usage_limits_for(credential_id).await {
            tracing::warn!("添加凭据后获取订阅等级失败（不影响凭据添加）: {}", e);
        }

        Ok(AddCredentialResponse {
            success: true,
            message: format!("凭据添加成功，ID: {}", credential_id),
            credential_id,
            email,
        })
    }

    /// 删除凭据
    pub fn delete_credential(&self, id: u64) -> Result<(), AdminServiceError> {
        self.token_manager
            .delete_credential(id)
            .map_err(|e| self.classify_delete_error(e, id))?;

        // 清理已删除凭据的余额缓存
        {
            let mut cache = self.balance_cache.lock();
            cache.remove(&id);
        }
        self.save_balance_cache();

        Ok(())
    }

    // ============ 余额缓存持久化 ============

    fn load_balance_cache_from(cache_path: &Option<PathBuf>) -> HashMap<u64, CachedBalance> {
        let path = match cache_path {
            Some(p) => p,
            None => return HashMap::new(),
        };

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };

        // 文件中使用字符串 key 以兼容 JSON 格式
        let map: HashMap<String, CachedBalance> = match serde_json::from_str(&content) {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("解析余额缓存失败，将忽略: {}", e);
                return HashMap::new();
            }
        };

        let now = Utc::now().timestamp() as f64;
        map.into_iter()
            .filter_map(|(k, v)| {
                let id = k.parse::<u64>().ok()?;
                // 丢弃超过 TTL 的条目
                if (now - v.cached_at) < BALANCE_CACHE_TTL_SECS as f64 {
                    Some((id, v))
                } else {
                    None
                }
            })
            .collect()
    }

    fn save_balance_cache(&self) {
        let path = match &self.cache_path {
            Some(p) => p,
            None => return,
        };

        // 快速 clone 数据后释放锁，减少锁持有时间
        let map: HashMap<String, CachedBalance> = {
            let cache = self.balance_cache.lock();
            cache
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect()
        };

        // 锁外执行序列化和文件 IO
        match serde_json::to_string_pretty(&map) {
            Ok(json) => {
                // 原子写入：先写临时文件，再重命名
                let tmp_path = path.with_extension("json.tmp");
                match std::fs::write(&tmp_path, json) {
                    Ok(_) => {
                        if let Err(e) = std::fs::rename(&tmp_path, path) {
                            tracing::warn!("原子重命名余额缓存失败: {}", e);
                            let _ = std::fs::remove_file(&tmp_path);
                        }
                    }
                    Err(e) => tracing::warn!("写入临时余额文件失败: {}", e),
                }
            }
            Err(e) => tracing::warn!("序列化余额缓存失败: {}", e),
        }
    }

    // ============ 错误分类 ============

    /// 分类简单操作错误（set_disabled, set_priority, reset_and_enable）
    fn classify_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("API Key 凭据无需刷新 Token") {
            AdminServiceError::InvalidCredential(msg)
        } else if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类余额查询错误（可能涉及上游 API 调用）
    fn classify_balance_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();

        // 1. 凭据不存在
        if msg.contains("不存在") {
            return AdminServiceError::NotFound { id };
        }

        // 2. 上游服务错误特征：HTTP 响应错误或网络错误
        let is_upstream_error =
            // HTTP 响应错误（来自 refresh_*_token 的错误消息）
            msg.contains("凭证已过期或无效") ||
            msg.contains("权限不足") ||
            msg.contains("已被限流") ||
            msg.contains("服务器错误") ||
            msg.contains("Token 刷新失败") ||
            msg.contains("暂时不可用") ||
            // 网络错误（reqwest 错误）
            msg.contains("error trying to connect") ||
            msg.contains("connection") ||
            msg.contains("timeout") ||
            msg.contains("timed out");

        if is_upstream_error {
            AdminServiceError::UpstreamError(msg)
        } else {
            // 3. 默认归类为内部错误（本地验证失败、配置错误等）
            // 包括：缺少 refreshToken、refreshToken 已被截断、无法生成 machineId 等
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类添加凭据错误
    fn classify_add_error(&self, e: anyhow::Error) -> AdminServiceError {
        let msg = e.to_string();

        // 凭据验证失败（refreshToken 无效、格式错误等）
        let is_invalid_credential = msg.contains("缺少 refreshToken")
            || msg.contains("refreshToken 为空")
            || msg.contains("refreshToken 已被截断")
            || msg.contains("缺少 kiroApiKey")
            || msg.contains("kiroApiKey 为空")
            || msg.contains("API Key 凭据无需刷新 Token")
            || msg.contains("凭据已存在")
            || msg.contains("refreshToken 或 kiroApiKey 重复")
            || msg.contains("凭证已过期或无效")
            || msg.contains("认证失败")
            || msg.contains("权限不足")
            || msg.contains("已被限流");

        if is_invalid_credential {
            AdminServiceError::InvalidCredential(msg)
        } else if msg.contains("error trying to connect")
            || msg.contains("connection")
            || msg.contains("timeout")
        {
            AdminServiceError::UpstreamError(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 分类删除凭据错误
    fn classify_delete_error(&self, e: anyhow::Error, id: u64) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id }
        } else if msg.contains("只能删除已禁用的凭据") || msg.contains("请先禁用凭据")
        {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    /// 批量导入 token.json
    ///
    /// 解析官方 token.json 格式，按 provider 字段自动映射 authMethod：
    /// - BuilderId/builder-id/idc → idc
    /// - Social/social → social
    pub async fn import_token_json(&self, req: ImportTokenJsonRequest) -> ImportTokenJsonResponse {
        let items = req.items.into_vec();
        let dry_run = req.dry_run;

        let mut results = Vec::with_capacity(items.len());
        let mut added = 0usize;
        let mut skipped = 0usize;
        let mut invalid = 0usize;

        for (index, item) in items.into_iter().enumerate() {
            let result = self.process_token_json_item(index, item, dry_run).await;
            match result.action {
                ImportAction::Added => added += 1,
                ImportAction::Skipped => skipped += 1,
                ImportAction::Invalid => invalid += 1,
            }
            results.push(result);
        }

        ImportTokenJsonResponse {
            summary: ImportSummary {
                parsed: results.len(),
                added,
                skipped,
                invalid,
            },
            items: results,
        }
    }

    /// 处理单个 token.json 项
    async fn process_token_json_item(
        &self,
        index: usize,
        item: TokenJsonItem,
        dry_run: bool,
    ) -> ImportItemResult {
        // 生成指纹（用于识别和去重）
        let fingerprint = Self::generate_fingerprint(&item);

        // 验证必填字段
        let refresh_token = match &item.refresh_token {
            Some(rt) if !rt.is_empty() => rt.clone(),
            _ => {
                return ImportItemResult {
                    index,
                    fingerprint,
                    action: ImportAction::Invalid,
                    reason: Some("缺少 refreshToken".to_string()),
                    credential_id: None,
                };
            }
        };

        // 映射 authMethod
        let auth_method = Self::map_auth_method(&item);

        // IdC 需要 clientId 和 clientSecret
        if auth_method == "idc" && (item.client_id.is_none() || item.client_secret.is_none()) {
            return ImportItemResult {
                index,
                fingerprint,
                action: ImportAction::Invalid,
                reason: Some(format!("{} 认证需要 clientId 和 clientSecret", auth_method)),
                credential_id: None,
            };
        }

        // 检查是否已存在（通过 refreshToken 前缀匹配）
        if self.token_manager.has_refresh_token_prefix(&refresh_token) {
            return ImportItemResult {
                index,
                fingerprint,
                action: ImportAction::Skipped,
                reason: Some("凭据已存在".to_string()),
                credential_id: None,
            };
        }

        // dry-run 模式只返回预览
        if dry_run {
            return ImportItemResult {
                index,
                fingerprint,
                action: ImportAction::Added,
                reason: Some("预览模式".to_string()),
                credential_id: None,
            };
        }

        // 实际添加凭据（trim + 空字符串转 None，与 set_region 逻辑一致）
        let region = item
            .region
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let api_region = item
            .api_region
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let new_cred = KiroCredentials {
            id: None,
            access_token: None,
            refresh_token: Some(refresh_token),
            kiro_api_key: None,
            profile_arn: None,
            expires_at: None,
            auth_method: Some(auth_method),
            client_id: item.client_id,
            client_secret: item.client_secret,
            priority: item.priority,
            region,
            api_region,
            machine_id: item.machine_id,
            endpoint: None,
            email: None,
            subscription_title: None,
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            groups: Vec::new(),
            source_channel: None,
            disabled: false,
            disable_reason: None,
            runtime_only: false,
        };

        match self.token_manager.add_credential(new_cred).await {
            Ok(credential_id) => ImportItemResult {
                index,
                fingerprint,
                action: ImportAction::Added,
                reason: None,
                credential_id: Some(credential_id),
            },
            Err(e) => ImportItemResult {
                index,
                fingerprint,
                action: ImportAction::Invalid,
                reason: Some(e.to_string()),
                credential_id: None,
            },
        }
    }

    /// 生成凭据指纹（用于识别）
    fn generate_fingerprint(item: &TokenJsonItem) -> String {
        // 使用 refreshToken 前 16 字符作为指纹
        // 使用 floor_char_boundary 安全截断，避免在多字节字符中间切割导致 panic
        item.refresh_token
            .as_ref()
            .map(|rt| {
                if rt.len() >= 16 {
                    let end = floor_char_boundary(rt, 16);
                    format!("{}...", &rt[..end])
                } else {
                    rt.clone()
                }
            })
            .unwrap_or_else(|| "(empty)".to_string())
    }

    /// 映射 provider/authMethod 到标准 authMethod
    fn map_auth_method(item: &TokenJsonItem) -> String {
        // 优先使用 authMethod 字段
        if let Some(auth) = &item.auth_method {
            let auth_lower = auth.to_lowercase();
            return match auth_lower.as_str() {
                "idc" | "builder-id" | "builderid" => "idc".to_string(),
                "social" => "social".to_string(),
                _ => auth_lower,
            };
        }

        // 回退到 provider 字段
        if let Some(provider) = &item.provider {
            let provider_lower = provider.to_lowercase();
            return match provider_lower.as_str() {
                "builderid" | "builder-id" | "idc" => "idc".to_string(),
                "social" => "social".to_string(),
                _ => "social".to_string(), // 默认 social
            };
        }

        // 默认 social
        "social".to_string()
    }

    // ============ 指标聚合 ============

    /// 获取指标概览
    pub fn metrics_summary(&self) -> MetricsSummaryResponse {
        let Some(collector) = &self.metrics else {
            return MetricsSummaryResponse {
                total_requests: 0,
                successful: 0,
                failed: 0,
                avg_latency_ms: 0.0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                window_size: 0,
            };
        };

        let events = collector.snapshot();
        let completed: Vec<_> = events
            .iter()
            .filter(|e| e.event_type == MetricEventType::RequestCompleted)
            .collect();

        let total_requests = completed.len();
        let successful = completed
            .iter()
            .filter(|e| e.status.as_deref() == Some("success"))
            .count();
        let failed = total_requests - successful;

        let (latency_sum, latency_count) =
            completed
                .iter()
                .fold((0u64, 0usize), |(sum, count), e| match e.latency_ms {
                    Some(ms) => (sum.saturating_add(ms), count + 1),
                    None => (sum, count),
                });
        let avg_latency_ms = if latency_count > 0 {
            latency_sum as f64 / latency_count as f64
        } else {
            0.0
        };

        let total_input_tokens: i64 = completed
            .iter()
            .filter_map(|e| e.input_tokens)
            .map(|t| t as i64)
            .sum();
        let total_output_tokens: i64 = completed
            .iter()
            .filter_map(|e| e.output_tokens)
            .map(|t| t as i64)
            .sum();

        MetricsSummaryResponse {
            total_requests,
            successful,
            failed,
            avg_latency_ms,
            total_input_tokens,
            total_output_tokens,
            window_size: events.len(),
        }
    }

    /// 获取按模型聚合的指标
    pub fn metrics_by_model(&self) -> Vec<ModelMetrics> {
        let Some(collector) = &self.metrics else {
            return Vec::new();
        };

        let events = collector.snapshot();

        // 按模型名称分组 RequestCompleted 事件
        let mut model_map: HashMap<String, (usize, u64, usize, i64, i64)> = HashMap::new();

        for event in &events {
            if event.event_type != MetricEventType::RequestCompleted {
                continue;
            }
            let model = event.model.as_deref().unwrap_or("unknown").to_string();
            let entry = model_map.entry(model).or_insert((0, 0, 0, 0, 0));
            entry.0 += 1; // request_count
            if let Some(ms) = event.latency_ms {
                entry.1 = entry.1.saturating_add(ms); // latency_sum
                entry.2 += 1; // latency_count
            }
            entry.3 += event.input_tokens.unwrap_or(0) as i64;
            entry.4 += event.output_tokens.unwrap_or(0) as i64;
        }

        let mut result: Vec<ModelMetrics> = model_map
            .into_iter()
            .map(
                |(
                    model,
                    (request_count, latency_sum, latency_count, input_tokens, output_tokens),
                )| {
                    ModelMetrics {
                        model,
                        request_count,
                        avg_latency_ms: if latency_count > 0 {
                            latency_sum as f64 / latency_count as f64
                        } else {
                            0.0
                        },
                        total_input_tokens: input_tokens,
                        total_output_tokens: output_tokens,
                    }
                },
            )
            .collect();

        // 按请求数降序排列
        result.sort_by_key(|b| std::cmp::Reverse(b.request_count));
        result
    }

    /// 获取按凭据聚合的指标
    pub fn metrics_by_credential(&self) -> Vec<CredentialMetrics> {
        let Some(collector) = &self.metrics else {
            return Vec::new();
        };

        let events = collector.snapshot();

        // 按凭据 ID 分组 RequestCompleted 事件
        let mut cred_map: HashMap<u64, (usize, usize, usize, u64, usize)> = HashMap::new();

        for event in &events {
            if event.event_type != MetricEventType::RequestCompleted {
                continue;
            }
            let Some(cred_id) = event.credential_id else {
                continue;
            };
            let entry = cred_map.entry(cred_id).or_insert((0, 0, 0, 0, 0));
            entry.0 += 1; // request_count
            if event.status.as_deref() == Some("success") {
                entry.1 += 1; // success_count
            } else {
                entry.2 += 1; // failure_count
            }
            if let Some(ms) = event.latency_ms {
                entry.3 = entry.3.saturating_add(ms); // latency_sum
                entry.4 += 1; // latency_count
            }
        }

        let mut result: Vec<CredentialMetrics> = cred_map
            .into_iter()
            .map(
                |(
                    credential_id,
                    (request_count, success_count, failure_count, latency_sum, latency_count),
                )| {
                    CredentialMetrics {
                        credential_id,
                        request_count,
                        success_count,
                        failure_count,
                        avg_latency_ms: if latency_count > 0 {
                            latency_sum as f64 / latency_count as f64
                        } else {
                            0.0
                        },
                    }
                },
            )
            .collect();

        // 按凭据 ID 排序
        result.sort_by_key(|c| c.credential_id);
        result
    }

    /// 获取当前代理配置（脱敏）
    pub fn get_proxy_config(&self) -> ProxyConfigResponse {
        let config = self.config.read();
        ProxyConfigResponse {
            proxy_url: config.proxy_url.clone(),
            has_credentials: config.proxy_username.is_some() && config.proxy_password.is_some(),
        }
    }

    /// 更新代理配置（热更新）
    pub async fn update_proxy_config(
        &self,
        req: UpdateProxyConfigRequest,
    ) -> Result<(), AdminServiceError> {
        // 1. 构建新的 ProxyConfig
        let new_proxy = if let Some(url) = &req.proxy_url {
            if url.trim().is_empty() {
                None
            } else {
                let mut proxy = ProxyConfig::new(url.trim());
                if let (Some(u), Some(p)) = (&req.proxy_username, &req.proxy_password)
                    && !u.trim().is_empty()
                    && !p.trim().is_empty()
                {
                    proxy = proxy.with_auth(u.trim(), p.trim());
                }
                // 如果未提供新认证信息，保留现有认证
                if proxy.username.is_none() {
                    let config = self.config.read();
                    if let (Some(u), Some(p)) = (&config.proxy_username, &config.proxy_password) {
                        proxy = proxy.with_auth(u, p);
                    }
                }
                Some(proxy)
            }
        } else {
            None
        };

        // 2. 先持久化配置（失败时不影响运行时状态）
        {
            let mut config = self.config.write();
            config.proxy_url = new_proxy.as_ref().map(|p| p.url.clone());
            config.proxy_username = new_proxy.as_ref().and_then(|p| p.username.clone());
            config.proxy_password = new_proxy.as_ref().and_then(|p| p.password.clone());
            config
                .save()
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        }

        // 3. 持久化成功后再应用运行时变更
        if let Some(provider) = &self.kiro_provider {
            provider
                .update_global_proxy(new_proxy.clone())
                .map_err(|e| AdminServiceError::InternalError(format!("代理配置无效: {}", e)))?;
        }

        // 4. 热更新 MultiTokenManager
        self.token_manager.update_proxy(new_proxy.clone());

        // 5. 同步更新 count_tokens 通道的代理配置
        crate::token::update_proxy(new_proxy);

        Ok(())
    }

    /// 获取全局配置
    pub fn get_global_config(&self) -> super::types::GlobalConfigResponse {
        let config = self.config.read();
        let c = self.compression_config.read();
        super::types::GlobalConfigResponse {
            region: config.region.clone(),
            credential_rpm: config.credential_rpm,
            credential_daily_max_requests: config.credential_daily_max_requests,
            prompt_cache_ttl_seconds: config.prompt_cache_ttl_seconds,
            prompt_cache_accounting_enabled: config.prompt_cache_accounting_enabled,
            default_endpoint: config.default_endpoint.clone(),
            compression: super::types::CompressionConfigResponse {
                enabled: c.enabled,
                whitespace_compression: c.whitespace_compression,
                thinking_strategy: c.thinking_strategy.clone(),
                tool_result_max_chars: c.tool_result_max_chars,
                tool_result_head_lines: c.tool_result_head_lines,
                tool_result_tail_lines: c.tool_result_tail_lines,
                tool_use_input_max_chars: c.tool_use_input_max_chars,
                tool_description_max_chars: c.tool_description_max_chars,
                max_history_turns: c.max_history_turns,
                max_history_chars: c.max_history_chars,
                max_request_body_bytes: c.max_request_body_bytes,
            },
        }
    }

    /// 更新全局配置
    pub async fn update_global_config(
        &self,
        req: super::types::UpdateGlobalConfigRequest,
    ) -> Result<(), AdminServiceError> {
        // 1. 先持久化配置（失败时不影响运行时状态）
        {
            let mut config = self.config.write();

            if let Some(region) = &req.region {
                let trimmed = region.trim();
                if trimmed.is_empty() {
                    return Err(AdminServiceError::InvalidRequest(
                        "Region 不能为空".to_string(),
                    ));
                }
                config.region = trimmed.to_string();
            }

            if let Some(rpm) = req.credential_rpm {
                config.credential_rpm = rpm;
            }

            if let Some(daily_max_requests) = req.credential_daily_max_requests {
                config.credential_daily_max_requests = daily_max_requests;
            }

            if let Some(ttl_seconds) = req.prompt_cache_ttl_seconds {
                if !is_supported_prompt_cache_ttl_seconds(ttl_seconds) {
                    return Err(AdminServiceError::InvalidRequest(
                        "Prompt Cache TTL 仅支持 300（5分钟）、3600（1小时）、7200（2小时）或 18000（5小时）"
                            .to_string(),
                    ));
                }
                config.prompt_cache_ttl_seconds = ttl_seconds;
            }

            if let Some(enabled) = req.prompt_cache_accounting_enabled {
                config.prompt_cache_accounting_enabled = enabled;
            }

            if let Some(ref endpoint) = req.default_endpoint {
                let trimmed = endpoint.trim();
                if trimmed.is_empty() {
                    return Err(AdminServiceError::InvalidRequest(
                        "默认 endpoint 不能为空".to_string(),
                    ));
                }
                if !self.known_endpoints.contains(trimmed) {
                    return Err(AdminServiceError::InvalidRequest(format!(
                        "未知的 endpoint: {}，可用值: {}",
                        trimmed,
                        self.known_endpoints
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
                config.default_endpoint = trimmed.to_string();
            }

            if let Some(c) = &req.compression {
                Self::apply_compression_fields(&mut config.compression, c);
            }

            config
                .save()
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        }

        // 2. 持久化成功后再应用运行时变更
        let config = self.config.read();

        // 热更新 region
        if req.region.is_some() {
            self.token_manager.update_region(config.region.clone());
        }

        // 热更新凭据级本地限流配置（组合 setter：单次 rebuild + 锁内原子发布）
        if req.credential_rpm.is_some() || req.credential_daily_max_requests.is_some() {
            self.token_manager.update_rate_limit_settings(
                config.credential_rpm,
                config.credential_daily_max_requests,
            );
        }

        // 热更新 default_endpoint
        if req.default_endpoint.is_some() {
            self.token_manager
                .update_default_endpoint(config.default_endpoint.clone());
            if let Some(provider) = &self.kiro_provider
                && let Err(e) = provider.update_default_endpoint(config.default_endpoint.clone())
            {
                tracing::warn!("热更新 KiroProvider default_endpoint 失败: {}", e);
            }
        }

        // 热更新 Prompt Cache 运行时配置
        if req.prompt_cache_ttl_seconds.is_some() || req.prompt_cache_accounting_enabled.is_some() {
            self.prompt_cache_runtime.write().update(
                req.prompt_cache_ttl_seconds,
                req.prompt_cache_accounting_enabled,
            );
        }

        // 热更新压缩配置到运行时 Arc<RwLock<CompressionConfig>>
        if let Some(c) = &req.compression {
            let mut runtime = self.compression_config.write();
            Self::apply_compression_fields(&mut runtime, c);
        }

        Ok(())
    }

    /// 将更新请求中的压缩字段应用到目标 CompressionConfig
    fn apply_compression_fields(
        target: &mut CompressionConfig,
        src: &super::types::UpdateCompressionConfigRequest,
    ) {
        if let Some(v) = src.enabled {
            target.enabled = v;
        }
        if let Some(v) = src.whitespace_compression {
            target.whitespace_compression = v;
        }
        if let Some(ref v) = src.thinking_strategy {
            target.thinking_strategy = v.clone();
        }
        if let Some(v) = src.tool_result_max_chars {
            target.tool_result_max_chars = v;
        }
        if let Some(v) = src.tool_result_head_lines {
            target.tool_result_head_lines = v;
        }
        if let Some(v) = src.tool_result_tail_lines {
            target.tool_result_tail_lines = v;
        }
        if let Some(v) = src.tool_use_input_max_chars {
            target.tool_use_input_max_chars = v;
        }
        if let Some(v) = src.tool_description_max_chars {
            target.tool_description_max_chars = v;
        }
        if let Some(v) = src.max_history_turns {
            target.max_history_turns = v;
        }
        if let Some(v) = src.max_history_chars {
            target.max_history_chars = v;
        }
        if let Some(v) = src.max_request_body_bytes {
            target.max_request_body_bytes = v;
        }
    }

    // ============ 风控冷却 ============

    /// 手动解除凭据的冷却状态
    pub fn clear_throttle(&self, id: u64) -> Result<(), AdminServiceError> {
        if self.token_manager.clear_credential_cooldown(id) {
            Ok(())
        } else {
            Err(AdminServiceError::NotFound { id })
        }
    }

    // ============ 凭据更新 ============

    /// 更新凭据可编辑字段
    pub fn update_credential(
        &self,
        id: u64,
        payload: super::types::UpdateCredentialRequest,
    ) -> Result<(), AdminServiceError> {
        let proxy_url = payload
            .proxy_url
            .map(|u| if u.trim().is_empty() { None } else { Some(u) });
        let proxy_username = payload
            .proxy_username
            .map(|u| if u.trim().is_empty() { None } else { Some(u) });
        let proxy_password = payload
            .proxy_password
            .map(|p| if p.trim().is_empty() { None } else { Some(p) });
        let email = payload
            .email
            .map(|e| if e.trim().is_empty() { None } else { Some(e) });
        let source_channel = payload
            .source_channel
            .map(|s| if s.trim().is_empty() { None } else { Some(s) });

        use crate::kiro::token_manager::CredentialFieldUpdate;
        self.token_manager
            .update_credential_fields(
                id,
                CredentialFieldUpdate {
                    email,
                    proxy_url,
                    proxy_username,
                    proxy_password,
                    groups: payload.groups,
                    source_channel,
                },
            )
            .map_err(|e| self.classify_proxy_error(e))
    }

    /// 更新已禁用凭据的 refreshToken
    pub fn update_refresh_token(
        &self,
        id: u64,
        payload: super::types::UpdateRefreshTokenRequest,
    ) -> Result<(), AdminServiceError> {
        self.token_manager
            .update_refresh_token(
                id,
                payload.refresh_token,
                payload.access_token,
                payload.expires_at,
            )
            .map_err(|e| self.classify_proxy_error(e))
    }

    /// 重置凭据的 success_count
    pub fn reset_success_count(&self, id: Option<u64>) -> Result<usize, AdminServiceError> {
        Ok(self.token_manager.reset_success_count(id))
    }

    // ============ 代理池 ============

    /// 获取代理池列表
    pub fn get_proxy_pool(&self) -> serde_json::Value {
        let entries = self.proxy_pool.list();
        let cred_proxies = self.token_manager.get_credential_proxy_urls();
        let proxies: Vec<ProxyPoolEntry> = entries
            .iter()
            .map(|e| {
                let credential_count = cred_proxies
                    .iter()
                    .filter(|(_, url)| url.as_deref() == Some(&e.url))
                    .count() as u32;
                ProxyPoolEntry {
                    id: e.id,
                    url: e.url.clone(),
                    label: e.label.clone(),
                    enabled: e.enabled,
                    credential_count,
                    health: e.health,
                    latency_ms: e.latency_ms,
                    last_checked_at: e.last_checked_at.clone(),
                    consecutive_failures: e.consecutive_failures,
                    auto_disabled: e.auto_disabled,
                }
            })
            .collect();
        serde_json::json!({
            "total": proxies.len(),
            "proxies": proxies,
        })
    }

    /// 添加代理到池中
    pub fn add_proxy(
        &self,
        url: String,
        label: Option<String>,
    ) -> Result<serde_json::Value, AdminServiceError> {
        let entry = self
            .proxy_pool
            .add(url, label)
            .map_err(|e| self.classify_proxy_error(e))?;
        let pool_entry = Self::new_proxy_pool_entry(&entry);
        serde_json::to_value(pool_entry)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))
    }

    /// 批量添加代理
    pub fn batch_add_proxies(
        &self,
        payload: super::types::BatchAddProxyRequest,
    ) -> (Vec<serde_json::Value>, Vec<String>) {
        let (added, errors) = self.proxy_pool.batch_add(payload.urls);
        let values: Vec<serde_json::Value> = added
            .iter()
            .filter_map(|e| serde_json::to_value(Self::new_proxy_pool_entry(e)).ok())
            .collect();
        (values, errors)
    }

    fn new_proxy_pool_entry(e: &super::proxy_pool::ProxyEntry) -> ProxyPoolEntry {
        ProxyPoolEntry {
            id: e.id,
            url: e.url.clone(),
            label: e.label.clone(),
            enabled: e.enabled,
            credential_count: 0,
            health: e.health,
            latency_ms: e.latency_ms,
            last_checked_at: e.last_checked_at.clone(),
            consecutive_failures: e.consecutive_failures,
            auto_disabled: e.auto_disabled,
        }
    }

    /// 删除代理
    pub fn delete_proxy(&self, id: u64) -> Result<(), AdminServiceError> {
        self.proxy_pool
            .delete(id)
            .map_err(|e| self.classify_proxy_error(e))
    }

    /// 设置代理启用/禁用
    pub fn set_proxy_enabled(&self, id: u64, enabled: bool) -> Result<(), AdminServiceError> {
        self.proxy_pool
            .set_enabled(id, enabled)
            .map_err(|e| self.classify_proxy_error(e))
    }

    /// 分配代理给凭据
    pub fn assign_proxy_to_credential(
        &self,
        id: u64,
        payload: super::types::AssignProxyRequest,
    ) -> Result<(), AdminServiceError> {
        let proxy_url = match payload.proxy_id {
            Some(proxy_id) => {
                let entries = self.proxy_pool.list();
                let entry = entries
                    .iter()
                    .find(|e| e.id == proxy_id)
                    .ok_or(AdminServiceError::NotFound { id: proxy_id })?;
                if !entry.enabled {
                    return Err(AdminServiceError::InvalidCredential(format!(
                        "代理 #{} 已禁用，无法分配",
                        proxy_id
                    )));
                }
                Some(entry.url.clone())
            }
            None => None,
        };
        self.token_manager
            .set_credential_proxy(id, proxy_url)
            .map_err(|e| self.classify_proxy_error(e))
    }

    /// 探测代理连通性
    pub async fn check_proxy(&self, id: u64) -> Result<serde_json::Value, AdminServiceError> {
        let entry = self
            .proxy_pool
            .check_one(id)
            .await
            .map_err(|e| self.classify_proxy_error(e))?;
        let resp = super::types::ProxyCheckResponse {
            id: entry.id,
            health: entry.health,
            latency_ms: entry.latency_ms,
            last_checked_at: entry.last_checked_at,
            enabled: entry.enabled,
            auto_disabled: entry.auto_disabled,
        };
        serde_json::to_value(resp).map_err(|e| AdminServiceError::InternalError(e.to_string()))
    }

    /// 全量代理健康检查
    pub async fn check_all_proxies(&self) -> serde_json::Value {
        let summary = self.proxy_pool.check_all().await;
        serde_json::json!({
            "healthy": summary.healthy,
            "unhealthy": summary.unhealthy,
            "autoDisabled": summary.auto_disabled,
        })
    }

    /// 轮询批量分配代理
    pub fn assign_proxies_round_robin(
        &self,
        credential_ids: Option<Vec<u64>>,
    ) -> Result<serde_json::Value, AdminServiceError> {
        let available_urls = self.proxy_pool.assignable_urls();
        if available_urls.is_empty() {
            return Err(AdminServiceError::InvalidCredential(
                "代理池中没有可分配的代理（需要启用且健康的代理）".to_string(),
            ));
        }

        let target_ids: Vec<u64> = match credential_ids {
            Some(ids) => ids,
            None => self.token_manager.all_enabled_credential_ids(),
        };

        let mut assigned = 0u32;
        let mut errors = Vec::new();
        for (i, &cred_id) in target_ids.iter().enumerate() {
            let proxy_url = &available_urls[i % available_urls.len()];
            match self
                .token_manager
                .set_credential_proxy(cred_id, Some(proxy_url.clone()))
            {
                Ok(()) => assigned += 1,
                Err(e) => errors.push(format!("凭据 #{}: {}", cred_id, e)),
            }
        }

        Ok(serde_json::json!({
            "assigned": assigned,
            "total": target_ids.len(),
            "proxyCount": available_urls.len(),
            "errors": errors,
        }))
    }

    fn classify_proxy_error(&self, e: anyhow::Error) -> AdminServiceError {
        let msg = e.to_string();
        if msg.contains("不存在") {
            AdminServiceError::NotFound { id: 0 }
        } else if msg.contains("已存在") || msg.contains("无效") || msg.contains("不能为空")
        {
            AdminServiceError::InvalidCredential(msg)
        } else {
            AdminServiceError::InternalError(msg)
        }
    }

    // ============ 负载均衡配置 ============

    /// 获取负载均衡模式
    pub fn get_load_balancing_mode(&self) -> super::types::LoadBalancingModeResponse {
        let config = self.config.read();
        super::types::LoadBalancingModeResponse {
            mode: config.load_balancing_mode.clone(),
        }
    }

    /// 设置负载均衡模式
    pub fn set_load_balancing_mode(
        &self,
        payload: super::types::SetLoadBalancingModeRequest,
    ) -> Result<super::types::LoadBalancingModeResponse, AdminServiceError> {
        let mode = payload.mode.trim().to_lowercase();
        if mode != "priority" && mode != "balanced" {
            return Err(AdminServiceError::InvalidRequest(
                "mode 必须是 priority 或 balanced".to_string(),
            ));
        }
        {
            let mut config = self.config.write();
            config.load_balancing_mode = mode.clone();
            config
                .save()
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        }
        Ok(super::types::LoadBalancingModeResponse { mode })
    }

    // ============ 账号级风控 ============

    /// 获取账号级风控故障转移配置
    pub fn get_account_throttle_config(&self) -> super::types::AccountThrottleConfigResponse {
        let config = self.config.read();
        super::types::AccountThrottleConfigResponse {
            failover: config.account_throttle_failover,
            cooldown_secs: config.account_throttle_cooldown_secs,
        }
    }

    /// 更新账号级风控故障转移配置
    pub fn set_account_throttle_config(
        &self,
        payload: super::types::SetAccountThrottleConfigRequest,
    ) -> Result<super::types::AccountThrottleConfigResponse, AdminServiceError> {
        {
            let mut config = self.config.write();
            if let Some(failover) = payload.failover {
                config.account_throttle_failover = failover;
            }
            if let Some(cooldown) = payload.cooldown_secs {
                if !(1..=86400).contains(&cooldown) {
                    return Err(AdminServiceError::InvalidRequest(
                        "cooldown_secs 必须在 1~86400 之间".to_string(),
                    ));
                }
                config.account_throttle_cooldown_secs = cooldown;
            }
            config
                .save()
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        }
        Ok(self.get_account_throttle_config())
    }

    // ============ 日志治理 ============

    /// 获取日志治理配置
    pub fn get_log_governance_config(&self) -> super::types::LogGovernanceConfigResponse {
        let config = self.config.read();
        super::types::LogGovernanceConfigResponse {
            trace_enabled: config.trace_enabled,
            trace_retention_days: config.trace_retention_days,
            usage_log_retention_days: config.usage_log_retention_days,
        }
    }

    /// 更新日志治理配置
    pub fn set_log_governance_config(
        &self,
        payload: super::types::SetLogGovernanceConfigRequest,
    ) -> Result<super::types::LogGovernanceConfigResponse, AdminServiceError> {
        {
            let mut config = self.config.write();
            if let Some(enabled) = payload.trace_enabled {
                config.trace_enabled = enabled;
            }
            if let Some(days) = payload.trace_retention_days {
                if !(1..=365).contains(&days) {
                    return Err(AdminServiceError::InvalidRequest(
                        "trace_retention_days 必须在 1~365 之间".to_string(),
                    ));
                }
                config.trace_retention_days = days;
            }
            if let Some(days) = payload.usage_log_retention_days {
                if !(1..=365).contains(&days) {
                    return Err(AdminServiceError::InvalidRequest(
                        "usage_log_retention_days 必须在 1~365 之间".to_string(),
                    ));
                }
                config.usage_log_retention_days = days;
            }
            config
                .save()
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        }
        Ok(self.get_log_governance_config())
    }

    // ============ 全局代理（新版） ============

    /// 获取当前全局代理 URL
    pub fn get_global_proxy(&self) -> Option<String> {
        self.config.read().proxy_url.clone()
    }

    /// 设置或清除全局代理
    pub async fn set_global_proxy(
        &self,
        proxy_url: Option<String>,
    ) -> Result<(), AdminServiceError> {
        self.update_proxy_config(UpdateProxyConfigRequest {
            proxy_url,
            proxy_username: None,
            proxy_password: None,
        })
        .await
    }

    // ============ Admin Key 持久化 ============

    /// 持久化 admin key 到 config.json
    pub fn persist_admin_key(&self, new_key: &str) -> Result<(), AdminServiceError> {
        let mut next_config = self.config.read().clone();
        next_config.admin_api_key = Some(new_key.to_string());
        next_config
            .save()
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        self.config.write().admin_api_key = Some(new_key.to_string());
        Ok(())
    }

    /// 持久化 api key 到 config.json
    pub fn persist_api_key(&self, new_key: &str) {
        let mut config = self.config.write();
        config.api_key = Some(new_key.to_string());
        if let Err(e) = config.save() {
            tracing::warn!("持久化 api_key 失败: {}", e);
        }
    }

    // ============ Social 登录（Portal PKCE OAuth）============

    pub async fn start_social_login(
        &self,
        req: super::types::StartSocialLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        let global_proxy = self.token_manager.proxy();
        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);
        let auth_endpoint = req
            .auth_endpoint
            .unwrap_or_else(|| social::KIRO_AUTH_ENDPOINT.to_string());

        let (code_verifier, code_challenge) = social::generate_pkce();
        let state = uuid::Uuid::new_v4().to_string();

        // 回调模式：配置 / 请求提供 callbackBaseUrl → 远程模式（公网回调路由自动接收）；
        // 否则本地模式（启动临时 TCP 端口，仅本机浏览器可达）。
        let remote_base = self.resolve_callback_base(req.callback_base_url.as_deref());
        let (redirect_uri, server_handle, remote_callback_tx, rx) = match remote_base.clone() {
            Some(base) => {
                let (tx, rx) = tokio::sync::oneshot::channel::<social::OAuthCallbackData>();
                (base, None, Some(Mutex::new(Some(tx))), rx)
            }
            None => {
                let (tx, rx) = tokio::sync::oneshot::channel::<social::OAuthCallbackData>();
                let (port, server_handle) = social::start_callback_server(tx)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
                (
                    format!("http://127.0.0.1:{}", port),
                    Some(server_handle),
                    None,
                    rx,
                )
            }
        };
        let portal_url = social::build_portal_url(&state, &code_challenge, &redirect_uri);
        let expires_at = Utc::now() + Duration::minutes(10);
        let session_id = uuid::Uuid::new_v4().to_string();

        let cred_template = KiroCredentials {
            auth_method: Some("social".to_string()),
            priority: req.priority,
            email: req.email,
            proxy_url: req.proxy_url,
            ..Default::default()
        };

        let session = SocialAuthSession {
            auth_endpoint,
            state,
            code_verifier,
            redirect_uri,
            expires_at,
            callback_rx: tokio::sync::Mutex::new(rx),
            cred_template,
            proxy,
            _server_handle: server_handle,
            remote_callback_tx,
            relogin_target_id: None,
        };
        self.social_sessions
            .lock()
            .insert(session_id.clone(), session);

        let resp = StartSocialLoginResponse {
            session_id,
            portal_url,
            expires_at: expires_at.to_rfc3339(),
            remote: remote_base.is_some(),
        };
        serde_json::to_value(resp).map_err(|e| AdminServiceError::InternalError(e.to_string()))
    }

    pub async fn poll_social_login(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, AdminServiceError> {
        use tokio::sync::oneshot::error::TryRecvError;

        enum PollOutcome {
            Expired,
            Closed,
            Pending,
            Received(social::OAuthCallbackData),
        }

        let outcome = {
            let sessions = self.social_sessions.lock();
            let Some(session) = sessions.get(session_id) else {
                return Err(AdminServiceError::NotFound { id: 0 });
            };
            if Utc::now() >= session.expires_at {
                PollOutcome::Expired
            } else {
                match session.callback_rx.try_lock() {
                    Ok(mut rx) => match rx.try_recv() {
                        Ok(data) => PollOutcome::Received(data),
                        Err(TryRecvError::Empty) => PollOutcome::Pending,
                        Err(TryRecvError::Closed) => PollOutcome::Closed,
                    },
                    Err(_) => PollOutcome::Pending,
                }
            }
        };

        match outcome {
            PollOutcome::Pending => {
                let resp = PollIdcLoginResponse::Pending;
                serde_json::to_value(resp)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))
            }
            PollOutcome::Expired => {
                self.social_sessions.lock().remove(session_id);
                let resp = PollIdcLoginResponse::Expired;
                serde_json::to_value(resp)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))
            }
            PollOutcome::Closed => {
                self.social_sessions.lock().remove(session_id);
                Err(AdminServiceError::InternalError(
                    "Social 登录回调服务器已关闭，请重新发起登录".to_string(),
                ))
            }
            PollOutcome::Received(callback) => {
                self.do_complete_social_login(session_id, callback).await
            }
        }
    }

    async fn do_complete_social_login(
        &self,
        session_id: &str,
        callback: social::OAuthCallbackData,
    ) -> Result<serde_json::Value, AdminServiceError> {
        {
            let sessions = self.social_sessions.lock();
            let s = sessions
                .get(session_id)
                .ok_or(AdminServiceError::NotFound { id: 0 })?;
            if callback.state != s.state {
                return Err(AdminServiceError::InternalError(
                    "OAuth state 不匹配，请重新发起登录".to_string(),
                ));
            }
        }

        let session = self
            .social_sessions
            .lock()
            .remove(session_id)
            .ok_or(AdminServiceError::NotFound { id: 0 })?;

        let config = self.token_manager.config();
        let full_redirect_uri = if callback.login_option.is_empty() {
            format!("{}{}", session.redirect_uri, callback.path)
        } else {
            format!(
                "{}{}?login_option={}",
                session.redirect_uri,
                callback.path,
                urlencoding::encode(&callback.login_option)
            )
        };

        let token = social::exchange_code_for_token(
            &session.auth_endpoint,
            &callback.code,
            &session.code_verifier,
            &full_redirect_uri,
            &config,
            session.proxy.as_ref(),
        )
        .await
        .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        if let Some(target_id) = session.relogin_target_id {
            let refresh_token = token.refresh_token.ok_or_else(|| {
                AdminServiceError::InternalError("Social 登录未返回 refreshToken".to_string())
            })?;
            self.do_relogin_update(target_id, refresh_token)
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
            tracing::info!("Social 重新登录成功，凭据 #{} Token 已更新", target_id);
            let resp = PollIdcLoginResponse::Success {
                credential_id: target_id,
            };
            return serde_json::to_value(resp)
                .map_err(|e| AdminServiceError::InternalError(e.to_string()));
        }

        let mut new_cred = session.cred_template;
        new_cred.access_token = Some(token.access_token);
        new_cred.refresh_token = token.refresh_token;
        new_cred.expires_at = token.expires_at.or_else(|| {
            token
                .expires_in
                .map(|secs| (Utc::now() + Duration::seconds(secs)).to_rfc3339())
        });
        if let Some(arn) = token.profile_arn {
            new_cred.profile_arn = Some(arn);
        }

        let credential_id = self
            .token_manager
            .add_credential(new_cred)
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        if let Err(e) = self.get_balance(credential_id).await {
            tracing::warn!("Social 登录后刷新余额失败: {}", e);
        }

        tracing::info!("Social 登录成功，已添加凭据 #{}", credential_id);
        let resp = PollIdcLoginResponse::Success { credential_id };
        serde_json::to_value(resp).map_err(|e| AdminServiceError::InternalError(e.to_string()))
    }

    pub async fn complete_social_login(
        &self,
        session_id: &str,
        code: String,
        state: String,
        login_option: String,
        path: String,
    ) -> Result<serde_json::Value, AdminServiceError> {
        {
            let sessions = self.social_sessions.lock();
            let s = sessions
                .get(session_id)
                .ok_or(AdminServiceError::NotFound { id: 0 })?;
            if Utc::now() >= s.expires_at {
                let resp = PollIdcLoginResponse::Expired;
                return serde_json::to_value(resp)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()));
            }
        }
        let callback = social::OAuthCallbackData {
            code,
            login_option,
            path,
            state,
        };
        self.do_complete_social_login(session_id, callback).await
    }

    /// 解析远程回调 base，优先级：`config.callbackBaseUrl`（显式覆盖 / 逃生口）> 请求自带 base > None（本地模式）。
    ///
    /// 返回 None 表示回落本地模式（都未提供 / 提供的值非法时记 warn）。
    fn resolve_callback_base(&self, req_base: Option<&str>) -> Option<String> {
        let raw = self
            .token_manager
            .config()
            .callback_base_url
            .as_deref()
            .map(str::to_string)
            .or_else(|| req_base.map(str::to_string))?;
        let trimmed = raw.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return None;
        }
        if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
            tracing::warn!(
                "callbackBaseUrl 非法（须以 http:// 或 https:// 开头），回落本地回调模式: {}",
                raw
            );
            return None;
        }
        Some(trimmed.to_string())
    }

    /// 公网 GET 回调路由调用：按 OAuth state 定位会话并投递回调数据。
    pub fn deliver_remote_social_callback(
        &self,
        state: &str,
        data: social::OAuthCallbackData,
    ) -> RemoteCallbackOutcome {
        let sessions = self.social_sessions.lock();
        let session_id = sessions
            .iter()
            .find_map(|(id, s)| (s.state == state).then_some(id.clone()));

        let Some(session_id) = session_id else {
            return RemoteCallbackOutcome::NotFound;
        };
        let session = sessions.get(&session_id).expect("刚查到的会话必然存在");
        if Utc::now() >= session.expires_at {
            return RemoteCallbackOutcome::Expired;
        }
        let tx_slot = match session.remote_callback_tx.as_ref() {
            Some(slot) => slot,
            None => return RemoteCallbackOutcome::NotFound,
        };
        let tx = tx_slot.lock().take();
        drop(sessions);
        match tx {
            Some(tx) => {
                if tx.send(data).is_ok() {
                    RemoteCallbackOutcome::Delivered
                } else {
                    RemoteCallbackOutcome::AlreadyCompleted
                }
            }
            None => RemoteCallbackOutcome::AlreadyCompleted,
        }
    }

    pub async fn start_social_relogin(
        &self,
        target_id: u64,
        req: super::types::StartSocialLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        {
            let snapshot = self.token_manager.snapshot();
            if !snapshot.entries.iter().any(|e| e.id == target_id) {
                return Err(AdminServiceError::NotFound { id: target_id });
            }
        }

        let global_proxy = self.token_manager.proxy();
        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);
        let auth_endpoint = req
            .auth_endpoint
            .unwrap_or_else(|| social::KIRO_AUTH_ENDPOINT.to_string());
        let (code_verifier, code_challenge) = social::generate_pkce();
        let state = uuid::Uuid::new_v4().to_string();
        // 回调模式同 start_social_login：远程模式走公网回调路由，本地模式走临时端口。
        let remote_base = self.resolve_callback_base(req.callback_base_url.as_deref());
        let (redirect_uri, server_handle, remote_callback_tx, rx) = match remote_base.clone() {
            Some(base) => {
                let (tx, rx) = tokio::sync::oneshot::channel::<social::OAuthCallbackData>();
                (base, None, Some(Mutex::new(Some(tx))), rx)
            }
            None => {
                let (tx, rx) = tokio::sync::oneshot::channel::<social::OAuthCallbackData>();
                let (port, server_handle) = social::start_callback_server(tx)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
                (
                    format!("http://127.0.0.1:{}", port),
                    Some(server_handle),
                    None,
                    rx,
                )
            }
        };
        let portal_url = social::build_portal_url(&state, &code_challenge, &redirect_uri);
        let expires_at = Utc::now() + Duration::minutes(10);
        let session_id = uuid::Uuid::new_v4().to_string();

        let session = SocialAuthSession {
            auth_endpoint,
            state,
            code_verifier,
            redirect_uri,
            expires_at,
            callback_rx: tokio::sync::Mutex::new(rx),
            cred_template: KiroCredentials::default(),
            proxy,
            _server_handle: server_handle,
            remote_callback_tx,
            relogin_target_id: Some(target_id),
        };
        self.social_sessions
            .lock()
            .insert(session_id.clone(), session);

        let resp = StartSocialLoginResponse {
            session_id,
            portal_url,
            expires_at: expires_at.to_rfc3339(),
            remote: remote_base.is_some(),
        };
        serde_json::to_value(resp).map_err(|e| AdminServiceError::InternalError(e.to_string()))
    }

    // ============ IdC 设备授权登录 ============

    pub async fn start_idc_login(
        &self,
        req: super::types::StartIdcLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        let config = self.token_manager.config();
        let global_proxy = self.token_manager.proxy();
        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);
        let start_url = req.start_url.as_deref().unwrap_or(BUILDER_ID_START_URL);

        let reg = idc::register_client(&req.region, start_url, &config, proxy.as_ref())
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        let device = idc::start_device_authorization(
            &req.region,
            start_url,
            &reg.client_id,
            &reg.client_secret,
            &config,
            proxy.as_ref(),
        )
        .await
        .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        let expires_at = Utc::now() + Duration::seconds(device.expires_in);
        let session_id = uuid::Uuid::new_v4().to_string();
        let cred_template = KiroCredentials {
            auth_method: Some("idc".to_string()),
            client_id: Some(reg.client_id.clone()),
            client_secret: Some(reg.client_secret.clone()),
            region: Some(req.region.clone()),
            priority: req.priority,
            email: req.email,
            proxy_url: req.proxy_url,
            ..Default::default()
        };
        let poll_interval = device.interval.max(5);
        let session = IdcAuthSession {
            region: req.region,
            client_id: reg.client_id,
            client_secret: reg.client_secret,
            device_code: device.device_code,
            expires_at,
            poll_interval,
            cred_template,
            proxy,
            relogin_target_id: None,
        };
        self.idc_sessions.lock().insert(session_id.clone(), session);

        let resp = StartIdcLoginResponse {
            session_id,
            user_code: device.user_code,
            verification_uri: device.verification_uri,
            verification_uri_complete: device.verification_uri_complete,
            expires_at: expires_at.to_rfc3339(),
            poll_interval,
        };
        serde_json::to_value(resp).map_err(|e| AdminServiceError::InternalError(e.to_string()))
    }

    pub async fn poll_idc_login(
        &self,
        session_id: &str,
    ) -> Result<serde_json::Value, AdminServiceError> {
        let (
            region,
            client_id,
            client_secret,
            device_code,
            _expires_at,
            proxy,
            cred_template,
            relogin_target_id,
        ) = {
            let sessions = self.idc_sessions.lock();
            let s = sessions
                .get(session_id)
                .ok_or(AdminServiceError::NotFound { id: 0 })?;
            if Utc::now() >= s.expires_at {
                let resp = PollIdcLoginResponse::Expired;
                return serde_json::to_value(resp)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()));
            }
            (
                s.region.clone(),
                s.client_id.clone(),
                s.client_secret.clone(),
                s.device_code.clone(),
                s.expires_at,
                s.proxy.clone(),
                s.cred_template.clone(),
                s.relogin_target_id,
            )
        };

        let config = self.token_manager.config();
        match idc::poll_token(
            &region,
            &client_id,
            &client_secret,
            &device_code,
            &config,
            proxy.as_ref(),
        )
        .await
        {
            idc::PollResult::Pending => {
                let resp = PollIdcLoginResponse::Pending;
                serde_json::to_value(resp)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))
            }
            idc::PollResult::Expired => {
                self.idc_sessions.lock().remove(session_id);
                let resp = PollIdcLoginResponse::Expired;
                serde_json::to_value(resp)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))
            }
            idc::PollResult::Error(e) => Err(AdminServiceError::InternalError(e.to_string())),
            idc::PollResult::Success(token) => {
                self.idc_sessions.lock().remove(session_id);
                if let Some(target_id) = relogin_target_id {
                    if let Some(refresh_token) = token.refresh_token {
                        self.do_relogin_update(target_id, refresh_token)
                            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
                    }
                    tracing::info!("IdC 重新登录成功，凭据 #{} Token 已更新", target_id);
                    let resp = PollIdcLoginResponse::Success {
                        credential_id: target_id,
                    };
                    return serde_json::to_value(resp)
                        .map_err(|e| AdminServiceError::InternalError(e.to_string()));
                }
                let mut new_cred = cred_template;
                new_cred.access_token = Some(token.access_token);
                new_cred.refresh_token = token.refresh_token;
                if let Some(secs) = token.expires_in {
                    new_cred.expires_at = Some((Utc::now() + Duration::seconds(secs)).to_rfc3339());
                }
                let credential_id = self
                    .token_manager
                    .add_credential(new_cred)
                    .await
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
                if let Err(e) = self.get_balance(credential_id).await {
                    tracing::warn!("IdC 登录后刷新余额失败: {}", e);
                }
                tracing::info!("IdC 设备授权登录成功，已添加凭据 #{}", credential_id);
                let resp = PollIdcLoginResponse::Success { credential_id };
                serde_json::to_value(resp)
                    .map_err(|e| AdminServiceError::InternalError(e.to_string()))
            }
        }
    }

    pub async fn start_idc_relogin(
        &self,
        target_id: u64,
        req: super::types::StartIdcLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        {
            let snapshot = self.token_manager.snapshot();
            if !snapshot.entries.iter().any(|e| e.id == target_id) {
                return Err(AdminServiceError::NotFound { id: target_id });
            }
        }
        let config = self.token_manager.config();
        let global_proxy = self.token_manager.proxy();
        let proxy = req
            .proxy_url
            .as_deref()
            .map(ProxyConfig::new)
            .or(global_proxy);
        let start_url = req.start_url.as_deref().unwrap_or(BUILDER_ID_START_URL);

        let reg = idc::register_client(&req.region, start_url, &config, proxy.as_ref())
            .await
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        let device = idc::start_device_authorization(
            &req.region,
            start_url,
            &reg.client_id,
            &reg.client_secret,
            &config,
            proxy.as_ref(),
        )
        .await
        .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;

        let expires_at = Utc::now() + Duration::seconds(device.expires_in);
        let session_id = uuid::Uuid::new_v4().to_string();
        let poll_interval = device.interval.max(5);
        let session = IdcAuthSession {
            region: req.region,
            client_id: reg.client_id,
            client_secret: reg.client_secret,
            device_code: device.device_code,
            expires_at,
            poll_interval,
            cred_template: KiroCredentials::default(),
            proxy,
            relogin_target_id: Some(target_id),
        };
        self.idc_sessions.lock().insert(session_id.clone(), session);

        let resp = StartIdcLoginResponse {
            session_id,
            user_code: device.user_code,
            verification_uri: device.verification_uri,
            verification_uri_complete: device.verification_uri_complete,
            expires_at: expires_at.to_rfc3339(),
            poll_interval,
        };
        serde_json::to_value(resp).map_err(|e| AdminServiceError::InternalError(e.to_string()))
    }

    fn do_relogin_update(&self, target_id: u64, refresh_token: String) -> anyhow::Result<()> {
        self.token_manager.set_disabled(target_id, true)?;
        self.token_manager
            .update_refresh_token(target_id, refresh_token, None, None)?;
        self.token_manager.reset_and_enable(target_id)?;
        Ok(())
    }

    /// 获取 token_manager 引用（供 handler 层直接操作）
    pub fn token_manager(&self) -> &MultiTokenManager {
        &self.token_manager
    }
}

fn credential_to_export_account(cred: KiroCredentials) -> Option<super::types::ExportedAccount> {
    let refresh_token = cred
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)?;

    fn non_empty(value: Option<String>) -> Option<String> {
        value
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    let auth_method = non_empty(cred.auth_method.clone()).map(|m| {
        if m.eq_ignore_ascii_case("idc")
            || m.eq_ignore_ascii_case("builder-id")
            || m.eq_ignore_ascii_case("iam")
        {
            "IdC".to_string()
        } else {
            "social".to_string()
        }
    });
    let is_idc = auth_method.as_deref() == Some("IdC");
    let idp = if is_idc { "BuilderId" } else { "Google" }.to_string();

    let status = if cred.disabled { "unknown" } else { "active" }.to_string();

    let expires_at_ms = cred
        .expires_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or(0);

    let subscription_type = match cred.subscription_title.as_deref() {
        Some(t) if t.to_uppercase().contains("FREE") => "Free",
        Some(t) if t.to_uppercase().contains("PRO+") => "Pro_Plus",
        Some(t) if t.to_uppercase().contains("PRO") => "Pro",
        _ => "Free",
    };
    let subscription = serde_json::json!({
        "type": subscription_type,
        "title": cred.subscription_title,
    });
    let now_ms = Utc::now().timestamp_millis();
    let usage = serde_json::json!({
        "current": 0, "limit": 0, "percentUsed": 0, "lastUpdated": now_ms,
    });

    let profile_arn = cred
        .profile_arn
        .as_deref()
        .filter(|arn| arn.contains(":profile/"))
        .map(str::to_string);

    let credentials = super::types::ExportedCredentials {
        access_token: non_empty(cred.access_token).unwrap_or_default(),
        csrf_token: String::new(),
        refresh_token: Some(refresh_token),
        client_id: non_empty(cred.client_id),
        client_secret: non_empty(cred.client_secret),
        region: non_empty(cred.region.clone()).or_else(|| non_empty(cred.api_region.clone())),
        expires_at: expires_at_ms,
        auth_method,
    };

    Some(super::types::ExportedAccount {
        id: uuid::Uuid::new_v4().to_string(),
        email: non_empty(cred.email).unwrap_or_default(),
        idp,
        machine_id: non_empty(cred.machine_id),
        profile_arn,
        credentials,
        subscription,
        usage,
        tags: Vec::new(),
        status,
        created_at: now_ms,
        last_used_at: now_ms,
    })
}

// ============ 在线更新 ============

impl AdminService {
    pub fn get_update_config(&self) -> UpdateConfigResponse {
        self.update_config.lock().response()
    }

    pub fn set_update_config(
        &self,
        req: SetUpdateConfigRequest,
    ) -> Result<UpdateConfigResponse, AdminServiceError> {
        let normalized_time = match req.auto_apply_time.as_deref() {
            Some(value) => Some(normalize_auto_apply_time(value)?),
            None => None,
        };

        let token_update: Option<Option<String>> = req.github_token.as_ref().map(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        });

        {
            let mut runtime = self.update_config.lock();
            if let Some(auto_apply) = req.auto_apply {
                runtime.auto_apply = auto_apply;
            }
            if let Some(time) = &normalized_time {
                runtime.auto_apply_time = time.clone();
            }
            if let Some(token) = &token_update {
                runtime.github_token = token.clone();
            }
        }

        {
            let mut config = self.config.write();
            if let Some(auto_apply) = req.auto_apply {
                config.update_auto_apply = auto_apply;
            }
            if let Some(time) = normalized_time {
                config.update_auto_apply_time = time;
            }
            if let Some(token) = token_update {
                config.github_token = token;
            }
            if let Err(e) = config.save() {
                tracing::warn!("持久化更新配置失败: {}", e);
            }
        }

        Ok(self.get_update_config())
    }

    pub async fn check_update(&self, force: bool) -> UpdateCheckInfo {
        if !force && let Some(cached) = self.update_check_cache.lock().clone() {
            let age = Utc::now()
                .signed_duration_since(cached.cached_at)
                .num_seconds();
            if age < UPDATE_CHECK_TTL_SECS {
                let mut info = cached.info.clone();
                info.cached = true;
                return info;
            }
        }

        match self.fetch_latest_release().await {
            Ok(info) => {
                self.update_check_cache.lock().replace(CachedUpdateCheck {
                    cached_at: Utc::now(),
                    info: info.clone(),
                });
                info
            }
            Err(err) => {
                let warning = format!("检查更新失败：{}", err);
                if let Some(cached) = self.update_check_cache.lock().clone() {
                    let mut info = cached.info.clone();
                    info.cached = true;
                    info.warning = Some(warning);
                    return info;
                }
                UpdateCheckInfo {
                    current_version: env!("CARGO_PKG_VERSION").to_string(),
                    latest_version: String::new(),
                    has_update: false,
                    build_type: BUILD_TYPE.to_string(),
                    release_name: None,
                    release_notes: None,
                    release_url: None,
                    published_at: None,
                    checked_at: Utc::now().to_rfc3339(),
                    cached: false,
                    warning: Some(warning),
                }
            }
        }
    }

    async fn fetch_latest_release(&self) -> Result<UpdateCheckInfo, AdminServiceError> {
        let url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            GITHUB_RELEASES_REPO
        );
        let token = self.update_config.lock().github_token.clone();
        let proxy = self.token_manager.proxy().map(|p| p.url.clone());
        let client = super::binary_update::build_http_client(proxy.as_deref())?;

        let mut req = client
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "kiro-rs-update-checker")
            .timeout(std::time::Duration::from_secs(15));
        if let Some(t) = token.as_deref() {
            let trimmed = t.trim();
            if !trimmed.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", trimmed));
            }
        }
        let resp = req.send().await.map_err(|e| {
            AdminServiceError::InternalError(format!("请求 GitHub API 失败: {}", e))
        })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AdminServiceError::InternalError(format!(
                "GitHub API 返回 {}: {}",
                status,
                body.chars().take(200).collect::<String>()
            )));
        }

        let release: GitHubRelease = resp.json().await.map_err(|e| {
            AdminServiceError::InternalError(format!("解析 GitHub release 失败: {}", e))
        })?;

        let current = env!("CARGO_PKG_VERSION").to_string();
        let latest_version = release.tag_name.trim().trim_start_matches('v').to_string();
        let has_update =
            !latest_version.is_empty() && compare_semver(&current, &latest_version).is_lt();

        Ok(UpdateCheckInfo {
            current_version: current,
            latest_version,
            has_update,
            build_type: BUILD_TYPE.to_string(),
            release_name: Some(release.name).filter(|v| !v.is_empty()),
            release_notes: Some(release.body).filter(|v| !v.is_empty()),
            release_url: Some(release.html_url).filter(|v| !v.is_empty()),
            published_at: Some(release.published_at).filter(|v| !v.is_empty()),
            checked_at: Utc::now().to_rfc3339(),
            cached: false,
            warning: None,
        })
    }

    async fn resolve_target_version(
        &self,
        require_update: bool,
    ) -> Result<String, AdminServiceError> {
        let info = self.check_update(true).await;
        if let Some(warn) = info.warning {
            return Err(AdminServiceError::InternalError(warn));
        }
        if info.latest_version.is_empty() {
            return Err(AdminServiceError::InternalError(
                "无法解析最新版本号（GitHub Releases 返回空）".to_string(),
            ));
        }
        if require_update && !info.has_update {
            return Err(AdminServiceError::InvalidRequest(format!(
                "当前已是最新版本 v{}，无需更新",
                info.current_version
            )));
        }
        Ok(info.latest_version)
    }

    pub async fn pull_update_image(&self) -> Result<ImageUpdateResponse, AdminServiceError> {
        let (proxy, token) = {
            let runtime = self.update_config.lock();
            (
                self.token_manager.proxy().map(|p| p.url.clone()),
                runtime.github_token.clone(),
            )
        };
        let exe = super::binary_update::current_executable()?;
        let version = self.resolve_target_version(false).await?;
        let staged = staged_binary_path(&exe, &version);

        let reused = staged.exists();
        if !reused {
            super::binary_update::download_release_binary(
                &version,
                proxy.as_deref(),
                token.as_deref(),
                &staged,
            )
            .await?;
        }
        super::binary_update::verify_staged_binary(&staged, &version)?;
        cleanup_other_staged(&exe, &version);

        Ok(ImageUpdateResponse {
            success: true,
            message: if reused {
                format!("v{} 已下载并校验，可直接执行「更新并重启」", version)
            } else {
                format!("已下载并校验 v{} 二进制，可直接执行「更新并重启」", version)
            },
            output: Some(format!(
                "{}: v{}\nstaged: {}",
                if reused { "reused" } else { "downloaded" },
                version,
                staged.display()
            )),
            applied: false,
            need_restart: false,
        })
    }

    pub async fn apply_image_update(&self) -> Result<ImageUpdateResponse, AdminServiceError> {
        let (proxy, token) = {
            let runtime = self.update_config.lock();
            (
                self.token_manager.proxy().map(|p| p.url.clone()),
                runtime.github_token.clone(),
            )
        };
        let exe = super::binary_update::current_executable()?;
        let version = self.resolve_target_version(true).await?;
        let staged = staged_binary_path(&exe, &version);

        let reused = staged.exists();
        if !reused {
            super::binary_update::download_release_binary(
                &version,
                proxy.as_deref(),
                token.as_deref(),
                &staged,
            )
            .await?;
        }
        let staged_metadata = super::binary_update::verify_staged_binary(&staged, &version)?;
        cleanup_other_staged(&exe, &version);

        let previous_version = env!("CARGO_PKG_VERSION").to_string();
        super::binary_update::install_binary(&exe, &staged)?;

        let prev_label = format!("v{}", previous_version);
        let applied_at = Utc::now().to_rfc3339();
        {
            let mut runtime = self.update_config.lock();
            runtime.previous_version = Some(prev_label.clone());
            runtime.last_applied_at = Some(applied_at.clone());
        }
        {
            let mut config = self.config.write();
            config.update_previous_version = Some(prev_label.clone());
            config.update_last_applied_at = Some(applied_at);
            if let Err(e) = config.save() {
                tracing::warn!("持久化更新状态失败: {}", e);
            }
        }

        super::binary_update::schedule_self_exit(std::time::Duration::from_secs(2));

        Ok(ImageUpdateResponse {
            success: true,
            message: format!(
                "已替换为 v{}，服务将在约 30 秒后完成重启",
                version
            ),
            output: Some(format!(
                "previous: v{}\n{}: v{}\nstaged_sha256: {}\nstaged_size: {}",
                previous_version,
                if reused { "reused-staged" } else { "installed" },
                version,
                staged_metadata.sha256,
                staged_metadata.size
            )),
            applied: true,
            need_restart: true,
        })
    }

    pub async fn rollback_image_update(&self) -> Result<ImageUpdateResponse, AdminServiceError> {
        let previous_label = self
            .update_config
            .lock()
            .previous_version
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                AdminServiceError::InvalidRequest(
                    "尚未记录可回退的版本，请先执行一次在线更新".to_string(),
                )
            })?
            .to_string();

        let exe = super::binary_update::current_executable()?;
        super::binary_update::restore_backup(&exe)?;
        cleanup_other_staged(&exe, "");

        {
            let mut runtime = self.update_config.lock();
            runtime.previous_version = None;
            runtime.last_applied_at = None;
        }
        {
            let mut config = self.config.write();
            config.update_previous_version = None;
            config.update_last_applied_at = None;
            if let Err(e) = config.save() {
                tracing::warn!("持久化回退状态失败: {}", e);
            }
        }

        super::binary_update::schedule_self_exit(std::time::Duration::from_secs(2));

        Ok(ImageUpdateResponse {
            success: true,
            message: format!(
                "已回退到 {}，服务将在约 30 秒后完成重启",
                previous_label
            ),
            output: Some(format!("rolled back to: {}", previous_label)),
            applied: true,
            need_restart: true,
        })
    }

    pub async fn check_rate_limit(&self, req: CheckRateLimitRequest) -> GitHubRateLimitInfo {
        let token = req
            .github_token
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .or_else(|| {
                self.update_config
                    .lock()
                    .github_token
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
            });
        let authenticated = token.is_some();

        let proxy = self.token_manager.proxy().map(|p| p.url.clone());
        let client = match super::binary_update::build_http_client(proxy.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                return GitHubRateLimitInfo {
                    valid: false,
                    authenticated,
                    limit: 0,
                    remaining: 0,
                    used: 0,
                    reset: 0,
                    login: None,
                    warning: Some(format!("构造 HTTP 客户端失败: {}", e)),
                };
            }
        };

        let mut req_builder = client
            .get("https://api.github.com/rate_limit")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "kiro-rs-update-checker")
            .timeout(std::time::Duration::from_secs(10));
        if let Some(t) = token.as_deref() {
            req_builder = req_builder.header("Authorization", format!("Bearer {}", t));
        }

        let resp = match req_builder.send().await {
            Ok(r) => r,
            Err(e) => {
                return GitHubRateLimitInfo {
                    valid: false,
                    authenticated,
                    limit: 0,
                    remaining: 0,
                    used: 0,
                    reset: 0,
                    login: None,
                    warning: Some(format!("请求 GitHub API 失败: {}", e)),
                };
            }
        };

        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return GitHubRateLimitInfo {
                valid: false,
                authenticated,
                limit: 0,
                remaining: 0,
                used: 0,
                reset: 0,
                login: None,
                warning: Some("GitHub Token 无效或已过期".to_string()),
            };
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return GitHubRateLimitInfo {
                valid: false,
                authenticated,
                limit: 0,
                remaining: 0,
                used: 0,
                reset: 0,
                login: None,
                warning: Some(format!(
                    "GitHub API 返回 {}: {}",
                    status,
                    body.chars().take(200).collect::<String>()
                )),
            };
        }

        let payload: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return GitHubRateLimitInfo {
                    valid: false,
                    authenticated,
                    limit: 0,
                    remaining: 0,
                    used: 0,
                    reset: 0,
                    login: None,
                    warning: Some(format!("解析 GitHub 响应失败: {}", e)),
                };
            }
        };

        let core = payload
            .get("resources")
            .and_then(|r| r.get("core"))
            .or_else(|| payload.get("rate"));
        let limit = core
            .and_then(|c| c.get("limit"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let remaining = core
            .and_then(|c| c.get("remaining"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let used = core
            .and_then(|c| c.get("used"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let reset = core
            .and_then(|c| c.get("reset"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let login = if authenticated {
            self.fetch_github_login(&client, token.as_deref()).await
        } else {
            None
        };

        GitHubRateLimitInfo {
            valid: true,
            authenticated,
            limit,
            remaining,
            used,
            reset,
            login,
            warning: None,
        }
    }

    async fn fetch_github_login(
        &self,
        client: &reqwest::Client,
        token: Option<&str>,
    ) -> Option<String> {
        let mut req = client
            .get("https://api.github.com/user")
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "kiro-rs-update-checker")
            .timeout(std::time::Duration::from_secs(10));
        if let Some(t) = token {
            req = req.header("Authorization", format!("Bearer {}", t));
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let payload: serde_json::Value = resp.json().await.ok()?;
        payload
            .get("login")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    pub fn start_auto_update_scheduler(self: &Arc<Self>) {
        let svc = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;

            let mut last_run_marker: Option<String> = None;
            let mut last_applied_version: Option<String> = None;

            loop {
                let runtime = svc.update_config.lock().clone();
                if runtime.auto_apply {
                    let target = parse_auto_apply_time(&runtime.auto_apply_time).ok();
                    if let Some((target_hour, target_minute)) = target {
                        let now = chrono::Local::now();
                        let date_minute_marker = format!(
                            "{}-{:02}:{:02}",
                            now.format("%Y-%m-%d"),
                            now.hour(),
                            now.minute()
                        );

                        let hit = now.hour() == target_hour && now.minute() == target_minute;
                        let already_ran_this_minute =
                            last_run_marker.as_deref() == Some(date_minute_marker.as_str());

                        if hit && !already_ran_this_minute {
                            last_run_marker = Some(date_minute_marker);
                            let info = svc.check_update(true).await;
                            if info.has_update
                                && !info.latest_version.is_empty()
                                && last_applied_version.as_deref()
                                    != Some(info.latest_version.as_str())
                            {
                                tracing::info!(
                                    "自动更新：到达计划时间 {}，发现新版本 {}（当前 {}），开始应用",
                                    runtime.auto_apply_time,
                                    info.latest_version,
                                    info.current_version
                                );
                                match svc.apply_image_update().await {
                                    Ok(res) => {
                                        tracing::info!("自动更新完成：{}", res.message);
                                        last_applied_version = Some(info.latest_version);
                                    }
                                    Err(e) => {
                                        tracing::warn!("自动更新失败：{}", e);
                                    }
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            "自动更新时间配置无效：{}，跳过本轮检查",
                            runtime.auto_apply_time
                        );
                    }
                }

                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anthropic::PromptCacheRuntime;
    use crate::kiro::endpoint::{CliEndpoint, IdeEndpoint, KiroEndpoint};
    use crate::kiro::model::credentials::KiroCredentials;
    use crate::kiro::provider::KiroProvider;
    use crate::kiro::token_manager::MultiTokenManager;
    use crate::model::config::{CompressionConfig, Config};
    use std::collections::HashSet;
    use std::env;
    use std::fs;

    fn create_test_service() -> AdminService {
        let config_path = env::temp_dir().join(format!(
            "kiro-admin-service-test-{}-{}.json",
            std::process::id(),
            fastrand::u64(..)
        ));

        let config = Arc::new(RwLock::new(Config::load(&config_path).unwrap()));
        let compression_config = Arc::new(RwLock::new(CompressionConfig::default()));
        let prompt_cache_runtime = Arc::new(RwLock::new(PromptCacheRuntime::new(300, true)));

        let credentials = KiroCredentials::default();
        let tm = Arc::new(
            MultiTokenManager::new(config.read().clone(), vec![credentials], None, None, false)
                .unwrap(),
        );

        let known_endpoints: HashSet<String> = vec![
            "ide".to_string(),
            "ide-runtime".to_string(),
            "cli".to_string(),
        ]
        .into_iter()
        .collect();

        let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        endpoints.insert("ide".to_string(), Arc::new(IdeEndpoint::new()));
        endpoints.insert("ide-runtime".to_string(), Arc::new(IdeEndpoint::runtime()));
        endpoints.insert("cli".to_string(), Arc::new(CliEndpoint::new()));
        let provider = Arc::new(KiroProvider::with_proxy(
            Arc::clone(&tm),
            None,
            endpoints,
            "ide".to_string(),
        ));

        AdminService::new(
            tm,
            Some(provider),
            config,
            compression_config,
            prompt_cache_runtime,
            None,
            known_endpoints,
        )
    }

    fn read_persisted_config(service: &AdminService) -> Config {
        let config_path = service.config.read().config_path().unwrap().to_path_buf();
        let content = fs::read_to_string(config_path).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    #[tokio::test]
    async fn test_update_global_config_default_endpoint_valid() {
        let service = create_test_service();

        let req = super::super::types::UpdateGlobalConfigRequest {
            region: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            prompt_cache_ttl_seconds: None,
            prompt_cache_accounting_enabled: None,
            default_endpoint: Some("cli".to_string()),
            compression: None,
        };

        let result = service.update_global_config(req).await;
        assert!(result.is_ok());

        let config = service.get_global_config();
        assert_eq!(config.default_endpoint, "cli");
        assert_eq!(service.token_manager.config().default_endpoint, "cli");

        let persisted = read_persisted_config(&service);
        assert_eq!(persisted.default_endpoint, "cli");
    }

    #[tokio::test]
    async fn test_update_global_config_default_endpoint_empty_rejected() {
        let service = create_test_service();

        let req = super::super::types::UpdateGlobalConfigRequest {
            region: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            prompt_cache_ttl_seconds: None,
            prompt_cache_accounting_enabled: None,
            default_endpoint: Some("".to_string()),
            compression: None,
        };

        let result = service.update_global_config(req).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("默认 endpoint 不能为空")
        );
    }

    #[tokio::test]
    async fn test_update_global_config_default_endpoint_whitespace_rejected() {
        let service = create_test_service();

        let req = super::super::types::UpdateGlobalConfigRequest {
            region: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            prompt_cache_ttl_seconds: None,
            prompt_cache_accounting_enabled: None,
            default_endpoint: Some("   ".to_string()),
            compression: None,
        };

        let result = service.update_global_config(req).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("默认 endpoint 不能为空")
        );
    }

    #[tokio::test]
    async fn test_update_global_config_default_endpoint_unknown_rejected() {
        let service = create_test_service();

        let req = super::super::types::UpdateGlobalConfigRequest {
            region: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            prompt_cache_ttl_seconds: None,
            prompt_cache_accounting_enabled: None,
            default_endpoint: Some("unknown".to_string()),
            compression: None,
        };

        let result = service.update_global_config(req).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("未知的 endpoint"));
        assert!(err_msg.contains("unknown"));
    }

    #[tokio::test]
    async fn test_update_global_config_default_endpoint_trimmed() {
        let service = create_test_service();

        let req = super::super::types::UpdateGlobalConfigRequest {
            region: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            prompt_cache_ttl_seconds: None,
            prompt_cache_accounting_enabled: None,
            default_endpoint: Some("  cli  ".to_string()),
            compression: None,
        };

        let result = service.update_global_config(req).await;
        assert!(result.is_ok());

        let config = service.get_global_config();
        assert_eq!(config.default_endpoint, "cli");
        assert_eq!(service.token_manager.config().default_endpoint, "cli");

        let persisted = read_persisted_config(&service);
        assert_eq!(persisted.default_endpoint, "cli");
    }

    #[tokio::test]
    async fn test_update_global_config_prompt_cache_ttl_accepts_long_values() {
        let service = create_test_service();

        for ttl_seconds in [7200, 18000] {
            let req = super::super::types::UpdateGlobalConfigRequest {
                region: None,
                credential_rpm: None,
                credential_daily_max_requests: None,
                prompt_cache_ttl_seconds: Some(ttl_seconds),
                prompt_cache_accounting_enabled: None,
                default_endpoint: None,
                compression: None,
            };

            let result = service.update_global_config(req).await;
            assert!(result.is_ok());

            let config = service.get_global_config();
            assert_eq!(config.prompt_cache_ttl_seconds, ttl_seconds);
            assert_eq!(
                service.prompt_cache_runtime.read().snapshot().ttl_seconds,
                ttl_seconds
            );

            let persisted = read_persisted_config(&service);
            assert_eq!(persisted.prompt_cache_ttl_seconds, ttl_seconds);
        }
    }

    #[tokio::test]
    async fn test_update_global_config_prompt_cache_ttl_rejects_unsupported_value() {
        let service = create_test_service();

        let initial_req = super::super::types::UpdateGlobalConfigRequest {
            region: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            prompt_cache_ttl_seconds: Some(3600),
            prompt_cache_accounting_enabled: None,
            default_endpoint: None,
            compression: None,
        };
        assert!(service.update_global_config(initial_req).await.is_ok());

        let req = super::super::types::UpdateGlobalConfigRequest {
            region: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            prompt_cache_ttl_seconds: Some(1234),
            prompt_cache_accounting_enabled: None,
            default_endpoint: None,
            compression: None,
        };

        let result = service.update_global_config(req).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Prompt Cache TTL 仅支持")
        );

        let config = service.get_global_config();
        assert_eq!(config.prompt_cache_ttl_seconds, 3600);
        assert_eq!(
            service.prompt_cache_runtime.read().snapshot().ttl_seconds,
            3600
        );

        let persisted = read_persisted_config(&service);
        assert_eq!(persisted.prompt_cache_ttl_seconds, 3600);
    }

    #[test]
    fn test_get_global_config_includes_default_endpoint() {
        let service = create_test_service();
        let config = service.get_global_config();
        assert_eq!(config.default_endpoint, "ide"); // Config::default() 的默认值
    }
}
