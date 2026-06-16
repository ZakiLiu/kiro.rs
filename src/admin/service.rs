//! Admin API 业务逻辑服务

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::anthropic::PromptCacheRuntime;
use crate::common::utf8::floor_char_boundary;
use crate::http_client::ProxyConfig;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::provider::KiroProvider;
use crate::kiro::token_manager::{CachedBalanceInfo, MultiTokenManager};
use crate::metrics::{MetricEventType, MetricsCollector};
use crate::model::config::{CompressionConfig, Config};
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

/// 余额缓存过期时间（秒），5 分钟
const BALANCE_CACHE_TTL_SECS: i64 = 300;

/// 缓存的余额条目（含时间戳）
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedBalance {
    /// 缓存时间（Unix 秒）
    cached_at: f64,
    /// 缓存的余额数据
    data: BalanceResponse,
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

        let proxy_pool_path = token_manager
            .cache_dir()
            .map(|d| d.join("proxy_pool.json"));
        let tls_backend = config.read().tls_backend;
        let proxy_pool = ProxyPoolManager::new(proxy_pool_path, tls_backend);

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
                }
            })
            .collect();

        // 按优先级排序（数字越小优先级越高）
        credentials.sort_by_key(|c| c.priority);

        CredentialsStatusResponse {
            total: snapshot.total,
            available: snapshot.available,
            ready: snapshot.ready,
            cooling: snapshot.cooling,
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

        Ok(BalanceResponse {
            id,
            subscription_title: usage.subscription_title().map(|s| s.to_string()),
            current_usage,
            usage_limit,
            remaining,
            usage_percentage,
            next_reset_at: usage.next_date_reset,
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
            disabled: false, // 新添加的凭据默认启用
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
            disabled: false,
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
                if !matches!(ttl_seconds, 300 | 3600) {
                    return Err(AdminServiceError::InvalidRequest(
                        "Prompt Cache TTL 仅支持 300（5分钟）或 3600（1小时）".to_string(),
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

    /// 手动解除凭据的账号级风控冷却
    pub fn clear_throttle(&self, _id: u64) -> Result<(), AdminServiceError> {
        // TODO: MultiTokenManager.clear_cooldown() 尚未在 CURRENT 项目实现
        Err(AdminServiceError::InternalError(
            "clear_throttle 尚未实现".to_string(),
        ))
    }

    // ============ 凭据更新 ============

    /// 更新凭据可编辑字段
    pub fn update_credential(
        &self,
        id: u64,
        payload: super::types::UpdateCredentialRequest,
    ) -> Result<(), AdminServiceError> {
        let proxy_url = payload.proxy_url.map(|u| {
            if u.trim().is_empty() { None } else { Some(u) }
        });
        let proxy_username = payload.proxy_username.map(|u| {
            if u.trim().is_empty() { None } else { Some(u) }
        });
        let proxy_password = payload.proxy_password.map(|p| {
            if p.trim().is_empty() { None } else { Some(p) }
        });
        let email = payload.email.map(|e| {
            if e.trim().is_empty() { None } else { Some(e) }
        });

        self.token_manager
            .update_credential_fields(id, email, proxy_url, proxy_username, proxy_password)
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
    pub fn reset_success_count(
        &self,
        _id: Option<u64>,
    ) -> Result<usize, AdminServiceError> {
        // TODO: MultiTokenManager.reset_success_count() 尚未在 CURRENT 项目实现
        Err(AdminServiceError::InternalError(
            "reset_success_count 尚未实现".to_string(),
        ))
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
            .filter_map(|e| {
                serde_json::to_value(Self::new_proxy_pool_entry(e)).ok()
            })
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
                    .ok_or_else(|| AdminServiceError::NotFound { id: proxy_id })?;
                if !entry.enabled {
                    return Err(AdminServiceError::InvalidCredential(
                        format!("代理 #{} 已禁用，无法分配", proxy_id),
                    ));
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
    pub async fn check_proxy(
        &self,
        id: u64,
    ) -> Result<serde_json::Value, AdminServiceError> {
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
        serde_json::to_value(resp)
            .map_err(|e| AdminServiceError::InternalError(e.to_string()))
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
        } else if msg.contains("已存在") || msg.contains("无效") || msg.contains("不能为空") {
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
    pub fn set_global_proxy(&self, proxy_url: Option<String>) -> Result<(), AdminServiceError> {
        let url = proxy_url
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        {
            let mut config = self.config.write();
            config.proxy_url = url;
            config
                .save()
                .map_err(|e| AdminServiceError::InternalError(e.to_string()))?;
        }
        Ok(())
    }

    // ============ Admin Key 持久化 ============

    /// 持久化 admin key 到 config.json
    pub fn persist_admin_key(&self, new_key: &str) {
        let mut config = self.config.write();
        config.admin_api_key = Some(new_key.to_string());
        if let Err(e) = config.save() {
            tracing::warn!("持久化 admin_api_key 失败: {}", e);
        }
    }

    /// 持久化 api key 到 config.json
    pub fn persist_api_key(&self, new_key: &str) {
        let mut config = self.config.write();
        config.api_key = Some(new_key.to_string());
        if let Err(e) = config.save() {
            tracing::warn!("持久化 api_key 失败: {}", e);
        }
    }

    // ============ Auth Flow STUBS ============
    // 以下方法依赖 auth/social.rs 和 auth/idc.rs，尚未移植到 CURRENT 项目。
    // 返回 501 Not Implemented 占位。

    /// 发起 IdC 设备授权登录
    pub async fn start_idc_login(
        &self,
        _payload: super::types::StartIdcLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        Err(AdminServiceError::InternalError(
            "IdC 登录模块尚未实现".to_string(),
        ))
    }

    /// 轮询 IdC 登录状态
    pub async fn poll_idc_login(
        &self,
        _session_id: &str,
    ) -> Result<serde_json::Value, AdminServiceError> {
        Err(AdminServiceError::InternalError(
            "IdC 登录模块尚未实现".to_string(),
        ))
    }

    /// 发起 Social 登录
    pub async fn start_social_login(
        &self,
        _payload: super::types::StartSocialLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        Err(AdminServiceError::InternalError(
            "Social 登录模块尚未实现".to_string(),
        ))
    }

    /// 轮询 Social 登录状态
    pub async fn poll_social_login(
        &self,
        _session_id: &str,
    ) -> Result<serde_json::Value, AdminServiceError> {
        Err(AdminServiceError::InternalError(
            "Social 登录模块尚未实现".to_string(),
        ))
    }

    /// 完成 Social 登录
    pub async fn complete_social_login(
        &self,
        _session_id: &str,
        _code: String,
        _state: String,
        _login_option: String,
        _path: String,
    ) -> Result<serde_json::Value, AdminServiceError> {
        Err(AdminServiceError::InternalError(
            "Social 登录模块尚未实现".to_string(),
        ))
    }

    /// 发起 Social 重新登录
    pub async fn start_social_relogin(
        &self,
        _id: u64,
        _payload: super::types::StartSocialLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        Err(AdminServiceError::InternalError(
            "Social 登录模块尚未实现".to_string(),
        ))
    }

    /// 发起 IdC 重新登录
    pub async fn start_idc_relogin(
        &self,
        _id: u64,
        _payload: super::types::StartIdcLoginRequest,
    ) -> Result<serde_json::Value, AdminServiceError> {
        Err(AdminServiceError::InternalError(
            "IdC 登录模块尚未实现".to_string(),
        ))
    }

    /// 获取 token_manager 引用（供 handler 层直接操作）
    pub fn token_manager(&self) -> &MultiTokenManager {
        &self.token_manager
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

        let known_endpoints: HashSet<String> = vec!["ide".to_string(), "cli".to_string()]
            .into_iter()
            .collect();

        let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        endpoints.insert("ide".to_string(), Arc::new(IdeEndpoint::new()));
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

    #[test]
    fn test_get_global_config_includes_default_endpoint() {
        let service = create_test_service();
        let config = service.get_global_config();
        assert_eq!(config.default_endpoint, "ide"); // Config::default() 的默认值
    }
}
