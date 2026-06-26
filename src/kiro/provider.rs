//! Kiro API Provider
//!
//! 核心组件，负责与 Kiro API 通信
//! 支持流式和非流式请求
//! 支持多凭据故障转移和重试

use chrono::{DateTime, Utc};
use parking_lot::{Mutex, RwLock};
use reqwest::Client;
use reqwest::header::HeaderMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::admin::trace_db::{self, TraceAttempt, truncate_snippet};
#[cfg(not(feature = "sensitive-logs"))]
use crate::common::utf8::floor_char_boundary;
use crate::http_client::{ProxyConfig, build_client};
use crate::kiro::cooldown::CooldownReason;
use crate::kiro::endpoint::{
    CliEndpoint, IDE_ENDPOINT_NAME, IdeEndpoint, KiroEndpoint, RequestContext,
};
use crate::kiro::machine_id;
use crate::kiro::model::credentials::KiroCredentials;
use crate::kiro::token_manager::{CallContext, MultiTokenManager};

/// 检测错误体中是否包含账户暂停信号（ASCII 大小写不敏感）
///
/// 覆盖 suspended/Suspended/SUSPENDED/TEMPORARILY_SUSPENDED 等变体
pub(crate) fn is_suspended_signal(s: &str) -> bool {
    s.to_ascii_lowercase().contains("suspended")
}

/// API 调用结果
pub struct ApiCallResult {
    pub response: reqwest::Response,
    pub credential_id: u64,
    pub attempts: Vec<TraceAttempt>,
}

/// MCP 调用结果
pub struct McpCallResult {
    pub response: reqwest::Response,
    pub credential_id: u64,
}

/// 总重试次数下限（凭据数不足时的保底值）
const MIN_TOTAL_RETRIES: usize = 3;

/// 大规模凭据场景的重试次数上限因子（凭据数/3）。
/// 小规模（≤15 个凭据）时直接遍历全部；超过时取 1/3 避免单请求耗时过长。
const RETRY_FRACTION_DIVISOR: usize = 3;

/// 小规模凭据上限：此数量以内的凭据集合，每个凭据至少获得一次重试机会
const SMALL_POOL_THRESHOLD: usize = 15;

/// 429 冷却默认时长（无 Retry-After header 时的基线冷却）
/// 旧值 60s 在级联 429 场景下导致所有凭据同时挂死 60s，系统完全瘫痪。
/// 降到 10s：Kiro 上游 429 通常 5-15s 恢复，10s 是合理的默认等待。
const DEFAULT_RATE_LIMIT_COOLDOWN_SECS: u64 = 10;

/// 429 冷却最小时长下限（防止 Retry-After=0 或极短值导致疯狂重试）
const MIN_RATE_LIMIT_COOLDOWN_SECS: u64 = 5;

/// 429 冷却最大时长上限（避免异常 Retry-After 把单号挂死太久）
const MAX_RATE_LIMIT_COOLDOWN_SECS: u64 = 120;

/// Kiro API Provider
///
/// 核心组件，负责与 Kiro API 通信
/// 支持多凭据故障转移和重试机制
pub struct KiroProvider {
    token_manager: Arc<MultiTokenManager>,
    /// 默认 client（无代理或全局代理）
    default_client: RwLock<Client>,
    /// 全局代理配置
    global_proxy: RwLock<Option<ProxyConfig>>,
    /// 凭据级代理 client 缓存（key: credential_id）
    client_cache: Mutex<HashMap<u64, Client>>,
    /// 端点实现注册表（第一阶段只注册 ide）
    endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
    /// 默认端点名称
    default_endpoint: RwLock<String>,
}

impl KiroProvider {
    fn default_endpoints() -> HashMap<String, Arc<dyn KiroEndpoint>> {
        let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        let ide: Arc<dyn KiroEndpoint> = Arc::new(IdeEndpoint::new());
        endpoints.insert(ide.name().to_string(), ide);
        let ide_runtime: Arc<dyn KiroEndpoint> = Arc::new(IdeEndpoint::runtime());
        endpoints.insert(ide_runtime.name().to_string(), ide_runtime);
        let cli: Arc<dyn KiroEndpoint> = Arc::new(CliEndpoint::new());
        endpoints.insert(cli.name().to_string(), cli);
        endpoints
    }

    /// 创建新的 KiroProvider 实例
    #[allow(dead_code)]
    pub fn new(token_manager: Arc<MultiTokenManager>) -> Self {
        Self::with_proxy(
            token_manager,
            None,
            Self::default_endpoints(),
            IDE_ENDPOINT_NAME.to_string(),
        )
    }

    /// 创建带代理配置的 KiroProvider 实例
    pub fn with_proxy(
        token_manager: Arc<MultiTokenManager>,
        proxy: Option<ProxyConfig>,
        endpoints: HashMap<String, Arc<dyn KiroEndpoint>>,
        default_endpoint: String,
    ) -> Self {
        assert!(
            endpoints.contains_key(&default_endpoint),
            "默认端点 {} 未在 endpoints 注册表中",
            default_endpoint
        );

        let default_client = build_client(proxy.as_ref(), 720, token_manager.config().tls_backend)
            .expect("创建 HTTP 客户端失败");

        Self {
            token_manager,
            default_client: RwLock::new(default_client),
            global_proxy: RwLock::new(proxy),
            client_cache: Mutex::new(HashMap::new()),
            endpoints,
            default_endpoint: RwLock::new(default_endpoint),
        }
    }

    /// 热更新全局代理配置
    ///
    /// 重建 default_client 并清空 client_cache
    pub fn update_global_proxy(&self, proxy: Option<ProxyConfig>) -> anyhow::Result<()> {
        let config = self.token_manager.config();
        let new_client = build_client(proxy.as_ref(), 720, config.tls_backend)?;

        *self.global_proxy.write() = proxy;
        *self.default_client.write() = new_client;
        self.client_cache.lock().clear();

        tracing::info!("全局代理配置已热更新，client_cache 已清空");
        Ok(())
    }

    /// 热更新默认 endpoint
    pub fn update_default_endpoint(&self, default_endpoint: String) -> anyhow::Result<()> {
        if !self.endpoints.contains_key(&default_endpoint) {
            return Err(anyhow::anyhow!("未知端点: {}", default_endpoint));
        }

        *self.default_endpoint.write() = default_endpoint;
        tracing::info!("默认 endpoint 已热更新");
        Ok(())
    }

    /// THINKING_SIGNATURE_INVALID 恢复路径：
    /// 剥离 history 中所有 assistantResponseMessage.reasoningContent 后重试。
    fn strip_reasoning_content_for_retry(request_body: &str) -> Option<String> {
        let mut parsed = serde_json::from_str::<serde_json::Value>(request_body).ok()?;
        if let Some(history) = parsed
            .pointer_mut("/conversationState/history")
            .and_then(|v| v.as_array_mut())
        {
            for msg in history.iter_mut() {
                if let Some(arm) = msg.get_mut("assistantResponseMessage")
                    && let Some(obj) = arm.as_object_mut()
                {
                    obj.remove("reasoningContent");
                }
            }
        }
        serde_json::to_string(&parsed).ok()
    }

    /// 从 Kiro 请求体中提取本次请求的模型 ID，用于凭据能力过滤。
    fn extract_model_id(request_body: &str) -> Option<String> {
        serde_json::from_str::<serde_json::Value>(request_body)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/conversationState/currentMessage/userInputMessage/modelId")
                    .or_else(|| value.get("model"))
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(ToOwned::to_owned)
            })
    }

    /// 获取凭据对应的 HTTP Client
    ///
    /// 优先使用凭据级代理，否则使用默认 client
    fn get_client_for_credential(&self, ctx: &CallContext) -> Client {
        let global_proxy = self.global_proxy.read().clone();
        let effective_proxy = ctx.credentials.effective_proxy(global_proxy.as_ref());

        if effective_proxy == global_proxy {
            return self.default_client.read().clone();
        }

        {
            let cache = self.client_cache.lock();
            if let Some(client) = cache.get(&ctx.id) {
                return client.clone();
            }
        }

        let config = self.token_manager.config();
        let client = build_client(effective_proxy.as_ref(), 720, config.tls_backend)
            .unwrap_or_else(|e| {
                tracing::warn!("创建凭据级代理 client 失败，使用默认 client: {}", e);
                self.default_client.read().clone()
            });

        {
            let mut cache = self.client_cache.lock();
            cache.insert(ctx.id, client.clone());
        }

        client
    }

    fn endpoint_for(&self, credentials: &KiroCredentials) -> anyhow::Result<Arc<dyn KiroEndpoint>> {
        let default_endpoint = self.default_endpoint.read();
        let name = credentials.effective_endpoint_name(Some(default_endpoint.as_str()));
        self.endpoints
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("未知端点: {}", name))
    }

    /// 获取 token_manager 的引用
    #[allow(dead_code)]
    pub fn token_manager(&self) -> &MultiTokenManager {
        &self.token_manager
    }

    /// 启动后台 Token 刷新任务
    ///
    /// 防止长时间空闲导致 Token 过期，避免请求到来时刷新失败导致凭据被永久禁用
    pub fn start_background_token_refresh(self: &Arc<Self>) {
        use crate::kiro::background_refresh::BackgroundRefreshConfig;
        let config = BackgroundRefreshConfig::default();
        let refresher = self.token_manager.start_background_refresh(config);
        tokio::spawn(async move {
            let _keep_alive = refresher;
            std::future::pending::<()>().await;
        });
    }

    /// 启动周期性 balance 刷新任务（P0#3 修复）。
    ///
    /// 实测：未修前 24h 0 条 balance refresh log，cache 完全是启动时冻结快照。
    /// 周期刷新让 LB 在长期运行中获得准确的 balance 信号，避免"陈旧最富"凭据被偏好。
    ///
    /// # 参数
    /// * `interval_secs` - 刷新周期（推荐 600s = 10min）
    pub fn start_periodic_balance_refresh(self: &Arc<Self>, interval_secs: u64) {
        let interval_secs = interval_secs.max(60);
        let provider = Arc::clone(self);
        tokio::spawn(async move {
            tracing::info!(
                interval_secs = interval_secs,
                "周期性 balance 刷新任务已启动"
            );
            // 启动后等一个周期再开始（启动时 initialize_balances 已经刷过一次）
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // 立即返回（第一个 tick）
            loop {
                ticker.tick().await;
                let tm = &provider.token_manager;
                let ids = tm.all_enabled_credential_ids();
                if ids.is_empty() {
                    continue;
                }
                tracing::debug!(count = ids.len(), "开始周期性 balance 刷新");
                // 串行 + 间隔，避免对上游突发压力
                for id in ids {
                    let balance_due = tm.should_refresh_balance(id);
                    // keepalive：凭据空闲超阈值时强制探测一次（补低余额 24h TTL 盲区）
                    let keepalive = !balance_due && tm.keepalive_due(id);
                    if !balance_due && !keepalive {
                        continue;
                    }
                    if keepalive {
                        tracing::info!("凭据 #{} 空闲超阈值，触发 keepalive 探测", id);
                    }
                    // 探测前记节流（成败一律），防网络抖动时每 tick 重试风暴
                    tm.mark_keepalive_probed(id);
                    match tm.get_usage_limits_for(id).await {
                        Ok(resp) => {
                            let used = resp.current_usage();
                            let remaining = resp.usage_limit() - used;
                            tm.update_balance_cache(id, remaining);

                            // KIRO PRO 超额检查
                            tm.check_pro_overuse_disable(id, resp.subscription_title(), used);
                            // 自动按订阅等级归类分组
                            tm.auto_assign_subscription_group(id, resp.subscription_title());

                            if remaining < 1.0 {
                                tracing::info!(
                                    "凭据 #{} 余额偏低 ({:.2})，保持可用（等待上游 402 判定）",
                                    id,
                                    remaining
                                );
                            }
                        }
                        Err(e) => {
                            tracing::debug!("凭据 #{} 周期 balance 刷新失败: {}", id, e);
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
            }
        });
    }

    /// 周期性尝试恢复被自动禁用的凭据
    ///
    /// 对以下类型的禁用凭据进行恢复尝试：
    /// - InsufficientBalance / QuotaExceeded: 余额探测成功即重新启用（remaining 如实写入缓存，含 0）
    /// - RefreshFailureLimit / FailureLimit: 尝试刷新 Token，成功则重新启用
    ///
    /// 不恢复：Manual, AuthenticationFailed, AccountSuspended
    ///
    /// 使用指数退避：基础间隔 5 分钟，每次失败翻倍，最大 30 分钟
    pub fn start_periodic_recovery(self: &Arc<Self>, interval_secs: u64) {
        let interval_secs = interval_secs.max(60);
        let provider = Arc::clone(self);
        tokio::spawn(async move {
            tracing::info!(interval_secs = interval_secs, "周期性凭据恢复任务已启动");
            let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // 第一个 tick 立即返回
            loop {
                ticker.tick().await;
                let tm = &provider.token_manager;
                let candidates = tm.get_recovery_candidates();
                if candidates.is_empty() {
                    continue;
                }
                tracing::info!(count = candidates.len(), "开始周期性凭据恢复检查");
                for (id, reason) in candidates {
                    match reason {
                        crate::kiro::token_manager::DisableReason::InsufficientBalance
                        | crate::kiro::token_manager::DisableReason::QuotaExceeded => {
                            // 尝试重新查询余额
                            match tm.get_usage_limits_for(id).await {
                                Ok(resp) => {
                                    let remaining = resp.usage_limit() - resp.current_usage();
                                    tm.recover_credential_inner(id);
                                    tm.update_balance_cache(id, remaining);
                                    tracing::info!(
                                        credential_id = id,
                                        remaining = remaining,
                                        "凭据探测成功，重新启用（上游允许超额，低余额/零余额也复活）"
                                    );
                                }
                                Err(e) => {
                                    tm.increment_recovery_attempts_inner(id);
                                    tracing::debug!("凭据 #{} 恢复检查失败（余额查询）: {}", id, e);
                                }
                            }
                        }
                        crate::kiro::token_manager::DisableReason::RefreshFailureLimit
                        | crate::kiro::token_manager::DisableReason::FailureLimit => {
                            // 尝试刷新 Token
                            match tm.force_refresh_token_for(id).await {
                                Ok(_) => {
                                    tracing::info!("凭据 #{} Token 刷新成功，已自动恢复", id);
                                    // force_refresh_token_for 内部已经恢复了凭据并持久化
                                }
                                Err(e) => {
                                    tm.increment_recovery_attempts_inner(id);
                                    tracing::debug!(
                                        "凭据 #{} 恢复检查失败（Token 刷新）: {}",
                                        id,
                                        e
                                    );
                                }
                            }
                        }
                        _ => {
                            // Manual, AuthenticationFailed, AccountSuspended, ModelUnavailable 不在此处理
                        }
                    }
                    // 间隔避免突发压力
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                // 批量操作完成后统一持久化
                tm.persist_if_needed();
            }
        });
    }

    /// 后台异步刷新余额缓存（如果需要）
    fn spawn_balance_refresh(&self, id: u64) {
        // 检查缓存是否需要刷新
        if !self.token_manager.should_refresh_balance(id) {
            return;
        }
        let tm = Arc::clone(&self.token_manager);
        tokio::spawn(async move {
            match tm.get_usage_limits_for(id).await {
                Ok(resp) => {
                    let remaining = resp.usage_limit() - resp.current_usage();
                    tm.update_balance_cache(id, remaining);
                    tracing::debug!("凭据 #{} 余额缓存已刷新: {:.2}", id, remaining);
                }
                Err(e) => {
                    tracing::warn!("凭据 #{} 余额刷新失败: {}", id, e);
                }
            }
        });
    }

    /// 发送非流式 API 请求
    ///
    /// 支持多凭据故障转移：
    /// - 400 Bad Request: 直接返回错误，不计入凭据失败
    /// - 401/403: 视为凭据/权限问题，计入失败次数并允许故障转移
    /// - 402 MONTHLY_REQUEST_COUNT: 视为额度用尽，禁用凭据并切换
    /// - 429/5xx/网络等瞬态错误: 重试但不禁用或切换凭据（避免误把所有凭据锁死）
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的请求体字符串
    ///
    /// # Returns
    /// 返回原始的 HTTP Response，不做解析
    pub async fn call_api(
        &self,
        request_body: &str,
        user_id: Option<&str>,
        group: Option<&str>,
    ) -> anyhow::Result<ApiCallResult> {
        self.call_api_with_retry(request_body, false, user_id, group)
            .await
    }

    pub async fn call_api_stream(
        &self,
        request_body: &str,
        user_id: Option<&str>,
        group: Option<&str>,
    ) -> anyhow::Result<ApiCallResult> {
        self.call_api_with_retry(request_body, true, user_id, group)
            .await
    }

    /// 发送 MCP API 请求
    ///
    /// 用于 WebSearch 等工具调用
    ///
    /// # Arguments
    /// * `request_body` - JSON 格式的 MCP 请求体字符串
    /// * `group` - 分组过滤（可选）
    ///
    /// # Returns
    /// 返回原始的 HTTP Response 以及实际使用的 credential_id
    pub async fn call_mcp(
        &self,
        request_body: &str,
        group: Option<&str>,
    ) -> anyhow::Result<McpCallResult> {
        self.call_mcp_with_retry(request_body, group).await
    }

    /// 内部方法：带重试逻辑的 MCP API 调用
    async fn call_mcp_with_retry(
        &self,
        request_body: &str,
        group: Option<&str>,
    ) -> anyhow::Result<McpCallResult> {
        let total_credentials = self.token_manager.total_count();
        let available = self.token_manager.available_count();
        if available == 0 {
            anyhow::bail!("没有可用的凭据");
        }
        let max_retries = available
            .min(SMALL_POOL_THRESHOLD)
            .max(total_credentials / RETRY_FRACTION_DIVISOR)
            .max(MIN_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut forced_token_refresh: HashSet<u64> = HashSet::new();
        let mut failed_ids: Vec<u64> = Vec::new();
        // 连续 429 计数器：连续 N 个不同凭据都返回 429 → 判定全局限流。
        let mut consecutive_429_count: usize = 0;
        let max_consecutive_429: usize = (available / 2).max(MIN_TOTAL_RETRIES);
        // 全局限流等待次数：检测到全局限流时先 sleep 等凭据冷却恢复再继续，而非立即 bail。
        let mut global_rate_limit_waits: usize = 0;
        const MAX_GLOBAL_RATE_LIMIT_WAITS: usize = 2;

        for attempt in 0..max_retries {
            // 获取调用上下文（支持排除已失败凭据）
            let ctx = match self
                .token_manager
                .acquire_context_with_group(&failed_ids, group)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    // 所有凭据均处于冷却 → 不再重试，直接返回（handlers.rs 会转为 429）
                    if e.to_string().contains("所有凭据均处于冷却/速率限制") {
                        return Err(e);
                    }
                    last_error = Some(e);
                    // 已 exclude 的凭据数 ≥ 当前可用集合，下一轮清空让 LB 重新挑选
                    if failed_ids.len() >= self.token_manager.available_count().max(1) {
                        failed_ids.clear();
                    }
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, &config)
                .ok_or_else(|| anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"))?;
            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(endpoint) => endpoint,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };
            let endpoint_name = endpoint.name();
            let request_ctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config: &config,
            };
            let url = endpoint.mcp_url(&request_ctx);
            let body = match endpoint.transform_mcp_body(request_body, &request_ctx) {
                Ok(body) => body,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };

            tracing::debug!(
                credential_id = %ctx.id,
                endpoint = %endpoint_name,
                "发送 MCP 请求"
            );
            let client = self.get_client_for_credential(&ctx);
            // Content-Type is endpoint-specific (CLI: application/x-amz-json-1.0,
            // IDE: application/json). Let decorate_mcp set it; reqwest's .header()
            // APPENDS on duplicate keys (we don't want two Content-Type values).
            let base_request = client.post(&url).body(body).header("Connection", "close");
            let request = endpoint.decorate_mcp(base_request, &request_ctx);
            #[cfg(feature = "sensitive-logs")]
            let _request_for_log = request.try_clone();

            // 发送请求
            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    tracing::warn!(
                        "MCP 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    last_error = Some(e.into());
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();
            let retry_after = Self::parse_retry_after(response.headers());

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                self.token_manager.record_api_success(ctx.id);
                tracing::info!(
                    credential_id = %ctx.id,
                    endpoint = %endpoint_name,
                    "MCP 请求成功"
                );
                return Ok(McpCallResult {
                    response,
                    credential_id: ctx.id,
                });
            }

            // 失败响应
            let body = response.text().await.unwrap_or_default();

            // 402 Payment Required
            if status.as_u16() == 402 {
                if endpoint.is_monthly_request_limit(&body) {
                    // 月度额度用尽 → 永久禁用凭据
                    let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                    if !has_available {
                        anyhow::bail!("MCP 请求失败（所有凭据已用尽）: {} {}", status, body);
                    }
                }
                // 其他 402（如 OVERAGE）→ 仅跳过此凭据，不禁用
                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                failed_ids.push(ctx.id);
                continue;
            }

            // 非 429 响应，重置连续 429 计数
            _ = consecutive_429_count;
            consecutive_429_count = 0;

            // 400 Bad Request
            if status.as_u16() == 400 {
                // profileArn 缺失：凭据级配置错误，永久禁用并故障转移
                if body.contains("profileArn is required") {
                    tracing::warn!(
                        "凭据 #{} 缺少 profileArn（永久禁用）: {} {}",
                        ctx.id, status, body
                    );
                    self.token_manager.mark_authentication_failed(ctx.id);
                    failed_ids.push(ctx.id);
                    last_error = Some(anyhow::anyhow!(
                        "MCP 请求失败（profileArn 缺失）: {} {}", status, body
                    ));
                    continue;
                }

                let is_too_long = Self::is_input_too_long(&body);
                // 输入过长错误：只记录请求体大小，不输出完整内容（太占空间且无调试价值）
                if is_too_long {
                    let body_bytes = request_body.len();
                    let estimated_tokens = Self::estimate_tokens(request_body);
                    tracing::error!(
                        status = %status,
                        response_body_bytes = body.len(),
                        request_url = %url,
                        request_body_bytes = body_bytes,
                        estimated_input_tokens = estimated_tokens,
                        "MCP 400 Bad Request - 输入上下文过长"
                    );
                } else {
                    // 其他 400 错误：记录请求信息以便调试
                    #[cfg(feature = "sensitive-logs")]
                    tracing::error!(
                        status = %status,
                        response_body = %body,
                        request_url = %url,
                        request_body_bytes = request_body.len(),
                        "MCP 400 Bad Request - 请求格式错误"
                    );
                    #[cfg(not(feature = "sensitive-logs"))]
                    tracing::error!(
                        status = %status,
                        response_body_bytes = body.len(),
                        request_url = %url,
                        request_body_bytes = request_body.len(),
                        "MCP 400 Bad Request - 请求格式错误"
                    );
                }
                #[cfg(feature = "sensitive-logs")]
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
                #[cfg(not(feature = "sensitive-logs"))]
                {
                    if is_too_long {
                        let body_bytes = request_body.len();
                        let estimated_tokens = Self::estimate_tokens(request_body);
                        anyhow::bail!(
                            "MCP 请求失败: {} Input is too long. (request_body_bytes={}, estimated_input_tokens={})",
                            status,
                            body_bytes,
                            estimated_tokens
                        );
                    }

                    let summary = Self::summarize_error_body(&body);
                    anyhow::bail!("MCP 请求失败: {} {}", status, summary);
                }
            }

            // 401/403 凭据问题
            if matches!(status.as_u16(), 401 | 403) {
                // 账户暂停 / TEMPORARILY_SUSPENDED：直接永久禁用
                if is_suspended_signal(&body) {
                    tracing::warn!(
                        "凭据 #{} 账户暂停（永久禁用）: {} {}",
                        ctx.id,
                        status,
                        body
                    );
                    self.token_manager.mark_authentication_failed(ctx.id);
                    failed_ids.push(ctx.id);
                    last_error = Some(anyhow::anyhow!(
                        "MCP 请求失败（账户暂停）: {} {}",
                        status,
                        body
                    ));
                    continue;
                }

                // bearer token 失效：刷新 token 并标记为已失败（换下个凭据重试）
                if endpoint.is_bearer_token_invalid(&body) && forced_token_refresh.insert(ctx.id) {
                    tracing::warn!(
                        "MCP 请求失败（Bearer token 无效，触发刷新，尝试 {}/{}）: {} {}",
                        attempt + 1,
                        max_retries,
                        status,
                        body
                    );
                    self.token_manager.invalidate_access_token(ctx.id);
                    failed_ids.push(ctx.id);
                    last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                    continue;
                }

                // 鉴权失败：直接永久禁用（不走失败计数，不参与自动恢复）
                tracing::warn!(
                    "凭据 #{} 鉴权失败（永久禁用）: {} {}",
                    ctx.id,
                    status,
                    body
                );
                self.token_manager.mark_authentication_failed(ctx.id);
                failed_ids.push(ctx.id);
                last_error = Some(anyhow::anyhow!(
                    "MCP 请求失败（鉴权失败）: {} {}",
                    status,
                    body
                ));
                continue;
            }

            if status.as_u16() == 429 {
                if Self::is_model_temporarily_unavailable(&body) {
                    tracing::warn!(
                        credential_id = %ctx.id,
                        "MCP 请求遇到 MODEL_TEMPORARILY_UNAVAILABLE（上游过载），按普通 429 处理"
                    );
                }

                // 429 策略：切下个凭据 retry + 平坦冷却分散后续请求
                let cooldown_duration =
                    retry_after.unwrap_or(Duration::from_secs(DEFAULT_RATE_LIMIT_COOLDOWN_SECS));
                self.token_manager.set_credential_cooldown_with_duration(
                    ctx.id,
                    CooldownReason::RateLimitExceeded,
                    Some(cooldown_duration),
                );
                self.token_manager.record_api_fail(ctx.id);
                consecutive_429_count += 1;
                tracing::warn!(
                    credential_id = %ctx.id,
                    cooldown_secs = cooldown_duration.as_secs(),
                    consecutive_429 = consecutive_429_count,
                    "MCP 请求触发 429，已设置 {}s 冷却，切换凭据重试",
                    cooldown_duration.as_secs()
                );

                if consecutive_429_count >= max_consecutive_429 {
                    if global_rate_limit_waits < MAX_GLOBAL_RATE_LIMIT_WAITS {
                        global_rate_limit_waits += 1;
                        tracing::warn!(
                            consecutive_429 = consecutive_429_count,
                            wait_round = global_rate_limit_waits,
                            max_wait_rounds = MAX_GLOBAL_RATE_LIMIT_WAITS,
                            "检测到全局限流，等待 {}s 让凭据冷却后重试（第 {}/{} 轮等待）",
                            cooldown_duration.as_secs(),
                            global_rate_limit_waits,
                            MAX_GLOBAL_RATE_LIMIT_WAITS
                        );
                        sleep(cooldown_duration).await;
                        consecutive_429_count = 0;
                        failed_ids.clear();
                        continue;
                    }
                    let retry_secs = cooldown_duration.as_millis().div_ceil(1000) as u64;
                    tracing::warn!(
                        consecutive_429 = consecutive_429_count,
                        "连续 {} 个凭据均返回 429，已等待 {} 轮仍未恢复，停止重试",
                        consecutive_429_count,
                        global_rate_limit_waits
                    );
                    anyhow::bail!(
                        "所有凭据均处于冷却/速率限制（retry_after_secs={}，原因：global_429_detected，来自凭据 #{}）",
                        retry_secs.max(5),
                        ctx.id
                    );
                }

                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                failed_ids.push(ctx.id);
                // 429 也需要 backoff，避免毫秒级疯狂轮转造成 thundering herd
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 瞬态错误
            if matches!(status.as_u16(), 408) || status.is_server_error() {
                tracing::warn!(
                    "MCP 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                self.token_manager.record_api_fail(ctx.id);

                if Self::is_model_temporarily_unavailable(&body) {
                    tracing::warn!(
                        credential_id = %ctx.id,
                        "MCP 5xx 遇到 MODEL_TEMPORARILY_UNAVAILABLE（上游过载），按瞬态错误重试"
                    );
                }

                last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx
            if status.is_client_error() {
                anyhow::bail!("MCP 请求失败: {} {}", status, body);
            }

            // 兜底
            last_error = Some(anyhow::anyhow!("MCP 请求失败: {} {}", status, body));
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("MCP 请求失败：已达到最大重试次数（{}次）", max_retries)
        }))
    }

    /// 内部方法：带重试逻辑的 API 调用
    ///
    /// 重试策略：
    /// - 总重试次数 = max(凭据数量, MIN_TOTAL_RETRIES)，确保所有凭据至少被尝试一次
    /// - 429 时切换凭据并设置平坦冷却，只有所有凭据都被限流才返回 429
    async fn call_api_with_retry(
        &self,
        request_body: &str,
        is_stream: bool,
        user_id: Option<&str>,
        group: Option<&str>,
    ) -> anyhow::Result<ApiCallResult> {
        let total_credentials = self.token_manager.total_count();
        let available = self.token_manager.available_count();
        if available == 0 {
            anyhow::bail!("没有可用的凭据");
        }
        let max_retries = available
            .min(SMALL_POOL_THRESHOLD)
            .max(total_credentials / RETRY_FRACTION_DIVISOR)
            .max(MIN_TOTAL_RETRIES);
        let mut last_error: Option<anyhow::Error> = None;
        let mut forced_token_refresh: HashSet<u64> = HashSet::new();
        // P0#1 修复：retry 链路必须排除上次失败的凭据，否则 acquire_context_for_user
        // 会因 affinity 命中而反复返回同一个绑定凭据。实测未修前 100 burst 切换率 0%。
        let mut failed_ids: Vec<u64> = Vec::new();
        let api_type = if is_stream { "流式" } else { "非流式" };
        let mut consecutive_429_count: usize = 0;
        let max_consecutive_429: usize = (available / 2).max(MIN_TOTAL_RETRIES);
        let mut global_rate_limit_waits: usize = 0;
        const MAX_GLOBAL_RATE_LIMIT_WAITS: usize = 2;
        let mut attempts: Vec<TraceAttempt> = Vec::new();
        let requested_model = Self::extract_model_id(request_body);

        for attempt in 0..max_retries {
            // 获取调用上下文（绑定 index、credentials、token），支持用户亲和性
            let ctx = match self
                .token_manager
                .acquire_context_for_user_with_model_and_group(
                    user_id,
                    &failed_ids,
                    requested_model.as_deref(),
                    group,
                )
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    // 所有凭据均处于冷却 → 不再重试，直接返回（handlers.rs 会转为 429）
                    if e.to_string().contains("所有凭据均处于冷却/速率限制") {
                        return Err(e);
                    }
                    last_error = Some(e);
                    // 已 exclude 的凭据数 ≥ 当前可用集合，下一轮清空让 LB 重新挑选
                    if failed_ids.len() >= self.token_manager.available_count().max(1) {
                        failed_ids.clear();
                    }
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let config = self.token_manager.config();
            let machine_id = machine_id::generate_from_credentials(&ctx.credentials, &config)
                .ok_or_else(|| anyhow::anyhow!("无法生成 machine_id，请检查凭证配置"))?;
            let endpoint = match self.endpoint_for(&ctx.credentials) {
                Ok(endpoint) => endpoint,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };
            let endpoint_name = endpoint.name();
            let request_ctx = RequestContext {
                credentials: &ctx.credentials,
                token: &ctx.token,
                machine_id: &machine_id,
                config: &config,
            };
            let url = endpoint.api_url(&request_ctx);
            let final_body = match endpoint.transform_api_body(request_body, &request_ctx) {
                Ok(body) => body,
                Err(e) => {
                    tracing::warn!("变换 endpoint 请求体失败，使用原始请求体: {}", e);
                    request_body.to_string()
                }
            };
            let final_body_for_log = final_body.clone();

            tracing::debug!(
                credential_id = %ctx.id,
                endpoint = %endpoint_name,
                "发送 {} API 请求",
                api_type
            );

            // 获取凭据对应的 client（支持凭据级代理）
            let client = self.get_client_for_credential(&ctx);
            // Content-Type is endpoint-specific (CLI: application/x-amz-json-1.0,
            // IDE: application/json). Let decorate_api set it; reqwest's .header()
            // APPENDS on duplicate keys.
            //
            // Wire-debug helper: in **debug builds only** (`cfg(debug_assertions)`),
            // set `KIRO_RS_CAPTURE=/some/dir` to dump the final post-transform
            // body to a timestamped JSON file. Useful for diff'ing against the
            // official kiro-cli `Q_LOG_LEVEL=trace` capture (see
            // docs/golden-gar-body.json) when re-aligning the protocol.
            // The env::var lookup is cached so the hot path stays one OnceLock
            // load.
            #[cfg(debug_assertions)]
            Self::wire_capture(&final_body);
            let base_request = client
                .post(&url)
                .body(final_body)
                .header("Connection", "close");
            let request = endpoint.decorate_api(base_request, &request_ctx);
            #[cfg(feature = "sensitive-logs")]
            let _request_for_log = request.try_clone();

            // 发送请求
            let attempt_start = Instant::now();
            let response = match request.send().await {
                Ok(resp) => resp,
                Err(e) => {
                    let duration_ms = attempt_start.elapsed().as_millis() as u64;
                    tracing::warn!(
                        "API 请求发送失败（尝试 {}/{}）: {}",
                        attempt + 1,
                        max_retries,
                        e
                    );
                    attempts.push(TraceAttempt {
                        attempt: attempt as u32,
                        credential_id: ctx.id,
                        endpoint: endpoint_name.to_string(),
                        http_status: None,
                        outcome: trace_db::outcome::NETWORK_ERROR.to_string(),
                        error_snippet: truncate_snippet(&e.to_string()),
                        duration_ms,
                    });
                    // 网络错误通常是上游/链路瞬态问题，不应导致"禁用凭据"或"切换凭据"
                    // （否则一段时间网络抖动会把所有凭据都误禁用，需要重启才能恢复）
                    last_error = Some(e.into());
                    // Round 11 修订：网络/链路错误**不** push failed_ids。
                    // 原因：DNS/TLS/上游域名瘫等链路抖动跟凭据无关，把所有凭据 push
                    // 后会触发"所有可用凭据均无法获取"误判 → 上游恢复后还要等容器自愈。
                    if attempt + 1 < max_retries {
                        sleep(Self::retry_delay(attempt)).await;
                    }
                    continue;
                }
            };

            let status = response.status();
            let retry_after = Self::parse_retry_after(response.headers());

            // 成功响应
            if status.is_success() {
                self.token_manager.report_success(ctx.id);
                self.token_manager.record_api_success(ctx.id);
                tracing::info!(
                    credential_id = %ctx.id,
                    endpoint = %endpoint_name,
                    "API 请求成功"
                );
                attempts.push(TraceAttempt {
                    attempt: attempt as u32,
                    credential_id: ctx.id,
                    endpoint: endpoint_name.to_string(),
                    http_status: Some(status.as_u16()),
                    outcome: trace_db::outcome::SUCCESS.to_string(),
                    error_snippet: None,
                    duration_ms: attempt_start.elapsed().as_millis() as u64,
                });
                // 后台异步刷新余额缓存
                self.spawn_balance_refresh(ctx.id);
                return Ok(ApiCallResult {
                    response,
                    credential_id: ctx.id,
                    attempts,
                });
            }

            // 失败响应：读取 body 用于日志/错误信息
            let body = response.text().await.unwrap_or_default();

            let attempt_outcome = match status.as_u16() {
                400 => trace_db::outcome::BAD_REQUEST,
                401 | 403 => trace_db::outcome::AUTH_FAILED,
                402 => trace_db::outcome::QUOTA_EXHAUSTED,
                429 => trace_db::outcome::ACCOUNT_THROTTLED,
                408 => trace_db::outcome::TRANSIENT,
                s if (500..600).contains(&s) => trace_db::outcome::TRANSIENT,
                _ => trace_db::outcome::UNKNOWN,
            };
            attempts.push(TraceAttempt {
                attempt: attempt as u32,
                credential_id: ctx.id,
                endpoint: endpoint_name.to_string(),
                http_status: Some(status.as_u16()),
                outcome: attempt_outcome.to_string(),
                error_snippet: truncate_snippet(&body),
                duration_ms: attempt_start.elapsed().as_millis() as u64,
            });

            // 402 Payment Required
            if status.as_u16() == 402 {
                if endpoint.is_monthly_request_limit(&body) {
                    // 月度额度用尽 → 永久禁用凭据
                    tracing::warn!(
                        "API 请求失败（额度已用尽，禁用凭据并切换，尝试 {}/{}）: {} {}",
                        attempt + 1,
                        max_retries,
                        status,
                        body
                    );
                    let has_available = self.token_manager.report_quota_exhausted(ctx.id);
                    self.token_manager.update_balance_cache(ctx.id, 0.0);
                    if !has_available {
                        anyhow::bail!(
                            "{} API 请求失败（所有凭据已用尽）: {} {}",
                            api_type,
                            status,
                            body
                        );
                    }
                }
                // 其他 402（如 OVERAGE）→ 设冷却跳过，不禁用
                tracing::warn!(
                    credential_id = %ctx.id,
                    "{} API 请求遇到 402 OVERAGE，设 300s 冷却跳过（不禁用）",
                    api_type
                );
                self.token_manager.set_credential_cooldown_with_duration(
                    ctx.id,
                    CooldownReason::QuotaExhausted,
                    Some(Duration::from_secs(300)),
                );
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                failed_ids.push(ctx.id);
                continue;
            }

            // 非 429 响应说明上游在处理请求（非全局限流），重置连续 429 计数
            let _ = std::mem::replace(&mut consecutive_429_count, 0);

            // 400 Bad Request
            if status.as_u16() == 400 {
                // profileArn 缺失：凭据级配置错误，永久禁用并故障转移
                if body.contains("profileArn is required") {
                    tracing::warn!(
                        "凭据 #{} 缺少 profileArn（永久禁用）: {} {}",
                        ctx.id, status, body
                    );
                    self.token_manager.mark_authentication_failed(ctx.id);
                    failed_ids.push(ctx.id);
                    last_error = Some(anyhow::anyhow!(
                        "{} API 请求失败（profileArn 缺失）: {} {}",
                        api_type, status, body
                    ));
                    continue;
                }

                // THINKING_SIGNATURE_INVALID: 模型更新导致 history 中的 thinking
                // signature 失效。自动剥离 reasoningContent 后重试一次（与官方 IDE 策略一致）。
                if body.contains("THINKING_SIGNATURE_INVALID") {
                    tracing::warn!(
                        "THINKING_SIGNATURE_INVALID detected, stripping reasoningContent and retrying"
                    );
                    if let Some(retry_body) =
                        Self::strip_reasoning_content_for_retry(&final_body_for_log)
                    {
                        let retry_request = client
                            .post(&url)
                            .body(retry_body)
                            .header("Connection", "close");
                        let retry_request = endpoint.decorate_api(retry_request, &request_ctx);
                        if let Ok(retry_resp) = retry_request.send().await {
                            if retry_resp.status().is_success() {
                                tracing::info!("THINKING_SIGNATURE_INVALID retry succeeded");
                                self.token_manager.report_success(ctx.id);
                                self.token_manager.record_api_success(ctx.id);
                                self.spawn_balance_refresh(ctx.id);
                                return Ok(ApiCallResult {
                                    response: retry_resp,
                                    credential_id: ctx.id,
                                    attempts,
                                });
                            }
                            tracing::warn!(
                                "THINKING_SIGNATURE_INVALID retry also failed: {}",
                                retry_resp.status()
                            );
                        }
                    }
                    // 重试失败，按正常 400 流程 bail
                }

                let is_too_long = Self::is_input_too_long(&body);
                // 输入过长错误：只记录请求体大小，不输出完整内容（太占空间且无调试价值）
                if is_too_long {
                    let body_bytes = final_body_for_log.len();
                    let estimated_tokens = Self::estimate_tokens(&final_body_for_log);
                    tracing::error!(
                        status = %status,
                        response_body_bytes = body.len(),
                        request_url = %url,
                        request_body_bytes = body_bytes,
                        estimated_input_tokens = estimated_tokens,
                        "400 Bad Request - 输入上下文过长"
                    );
                } else {
                    // 其他 400 错误：记录请求信息以便调试
                    #[cfg(feature = "sensitive-logs")]
                    tracing::error!(
                        status = %status,
                        response_body = %body,
                        request_url = %url,
                        request_body = %Self::truncate_body_for_log(&final_body_for_log, 1200),
                        "400 Bad Request - 请求格式错误"
                    );
                    #[cfg(not(feature = "sensitive-logs"))]
                    tracing::error!(
                        status = %status,
                        response_body_bytes = body.len(),
                        request_url = %url,
                        request_body_bytes = final_body_for_log.len(),
                        "400 Bad Request - 请求格式错误"
                    );
                }
                #[cfg(feature = "sensitive-logs")]
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
                #[cfg(not(feature = "sensitive-logs"))]
                {
                    // 对用户保留可区分的错误信息（例如 Input is too long），但避免返回过长内容。
                    if is_too_long {
                        let body_bytes = final_body_for_log.len();
                        let estimated_tokens = Self::estimate_tokens(&final_body_for_log);
                        anyhow::bail!(
                            "{} API 请求失败: {} Input is too long. (request_body_bytes={}, estimated_input_tokens={})",
                            api_type,
                            status,
                            body_bytes,
                            estimated_tokens
                        );
                    }

                    let summary = Self::summarize_error_body(&body);
                    anyhow::bail!("{} API 请求失败: {} {}", api_type, status, summary);
                }
            }

            // 401/403 - 凭据/权限问题：直接永久禁用
            if matches!(status.as_u16(), 401 | 403) {
                // 账户暂停 / TEMPORARILY_SUSPENDED：直接永久禁用
                if is_suspended_signal(&body) {
                    tracing::warn!(
                        "凭据 #{} 账户暂停（永久禁用）: {} {}",
                        ctx.id,
                        status,
                        body
                    );
                    self.token_manager.mark_authentication_failed(ctx.id);
                    failed_ids.push(ctx.id);
                    last_error = Some(anyhow::anyhow!(
                        "{} API 请求失败（账户暂停）: {} {}",
                        api_type,
                        status,
                        body
                    ));
                    continue;
                }

                // bearer token 失效：刷新 token 并标记为已失败（换下个凭据重试）
                if endpoint.is_bearer_token_invalid(&body) && forced_token_refresh.insert(ctx.id) {
                    tracing::warn!(
                        "API 请求失败（Bearer token 无效，触发刷新，尝试 {}/{}）: {} {}",
                        attempt + 1,
                        max_retries,
                        status,
                        body
                    );
                    self.token_manager.invalidate_access_token(ctx.id);
                    failed_ids.push(ctx.id);
                    last_error = Some(anyhow::anyhow!(
                        "{} API 请求失败: {} {}",
                        api_type,
                        status,
                        body
                    ));
                    continue;
                }

                // 鉴权失败：直接永久禁用（不走失败计数，不参与自动恢复）
                tracing::warn!(
                    "凭据 #{} 鉴权失败（永久禁用）: {} {}",
                    ctx.id,
                    status,
                    body
                );
                self.token_manager.mark_authentication_failed(ctx.id);
                failed_ids.push(ctx.id);
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败（鉴权失败）: {} {}",
                    api_type,
                    status,
                    body
                ));
                continue;
            }

            // 429 + suspicious activity = 账号级临时风控
            if status.as_u16() == 429
                && self.token_manager.get_account_throttle_failover()
                && endpoint.is_account_throttled(&body)
            {
                let cooldown_secs = self
                    .token_manager
                    .get_account_throttle_cooldown_secs()
                    .max(1);
                let cooldown = Duration::from_secs(cooldown_secs);
                tracing::warn!(
                    credential_id = %ctx.id,
                    cooldown_secs = cooldown_secs,
                    "{} API 请求失败（账号级风控，凭据 #{} 冷却 {}s 并切换）",
                    api_type, ctx.id, cooldown_secs
                );
                let remaining = self
                    .token_manager
                    .report_account_throttled(ctx.id, cooldown);
                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败（账号级风控，凭据 #{} 已冷却 {}s）: {} {}",
                    api_type,
                    ctx.id,
                    cooldown_secs,
                    status,
                    body
                ));
                if remaining == 0 {
                    anyhow::bail!(
                        "所有凭据均处于冷却/速率限制（retry_after_secs={}，原因：account_throttle，来自凭据 #{}）",
                        cooldown_secs,
                        ctx.id
                    );
                }
                failed_ids.push(ctx.id);
                continue;
            }

            // 客户端请求格式错误：不重试、不切换凭据、立即终止（避免 503 风暴）
            if endpoint.is_client_validation_error(&body) {
                tracing::warn!(
                    "{} API 请求失败（客户端请求格式错误，不重试）: {} {}",
                    api_type,
                    status,
                    body
                );
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 524 / 网关超时：快速返回，让客户端下次重连
            if status.as_u16() == 524 || endpoint.is_gateway_timeout(&body) {
                tracing::warn!(
                    "{} API 请求失败（上游网关超时，不重试）: {} {}",
                    api_type,
                    status,
                    body
                );
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            if status.as_u16() == 429 {
                // MODEL_TEMPORARILY_UNAVAILABLE 不再触发全局熔断，改为普通 429 冷却。
                if Self::is_model_temporarily_unavailable(&body) {
                    tracing::warn!(
                        credential_id = %ctx.id,
                        "{} API 请求遇到 MODEL_TEMPORARILY_UNAVAILABLE（上游过载），按普通 429 处理",
                        api_type
                    );
                }

                // 429 策略（Round 8 修订）：切下个凭据 retry + 平坦冷却分散后续请求。
                // - 本轮 retry 强制 exclude 此凭据
                // - 设置平坦冷却（不累计 trigger_count，无指数雪球）分散后续独立请求
                // - 所有凭据都被限流时 acquire_context 会 bail，handlers.rs 返回 429
                let cooldown_duration =
                    retry_after.unwrap_or(Duration::from_secs(DEFAULT_RATE_LIMIT_COOLDOWN_SECS));
                self.token_manager.set_credential_cooldown_with_duration(
                    ctx.id,
                    CooldownReason::RateLimitExceeded,
                    Some(cooldown_duration),
                );
                self.token_manager.record_api_fail(ctx.id);
                consecutive_429_count += 1;
                tracing::warn!(
                    credential_id = %ctx.id,
                    cooldown_secs = cooldown_duration.as_secs(),
                    consecutive_429 = consecutive_429_count,
                    "{} API 请求触发 429，已设置 {}s 冷却，切换凭据重试",
                    api_type,
                    cooldown_duration.as_secs()
                );

                // 连续 N 个不同凭据都 429 → 全局限流，等待冷却后重试而非立即 bail
                if consecutive_429_count >= max_consecutive_429 {
                    if global_rate_limit_waits < MAX_GLOBAL_RATE_LIMIT_WAITS {
                        global_rate_limit_waits += 1;
                        tracing::warn!(
                            consecutive_429 = consecutive_429_count,
                            wait_round = global_rate_limit_waits,
                            max_wait_rounds = MAX_GLOBAL_RATE_LIMIT_WAITS,
                            "检测到全局限流，等待 {}s 让凭据冷却后重试（第 {}/{} 轮等待）",
                            cooldown_duration.as_secs(),
                            global_rate_limit_waits,
                            MAX_GLOBAL_RATE_LIMIT_WAITS
                        );
                        sleep(cooldown_duration).await;
                        consecutive_429_count = 0;
                        failed_ids.clear();
                        continue;
                    }
                    let retry_secs = cooldown_duration.as_millis().div_ceil(1000) as u64;
                    tracing::warn!(
                        consecutive_429 = consecutive_429_count,
                        "连续 {} 个凭据均返回 429，已等待 {} 轮仍未恢复，停止重试",
                        consecutive_429_count,
                        global_rate_limit_waits
                    );
                    anyhow::bail!(
                        "所有凭据均处于冷却/速率限制（retry_after_secs={}，原因：global_429_detected，来自凭据 #{}）",
                        retry_secs.max(5),
                        ctx.id
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                // 429 不 push failed_ids（cooldown 自然排除），用慢退避给配额恢复时间
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay_throttle(attempt)).await;
                }
                continue;
            }

            // 408/5xx - 瞬态上游错误：重试但不禁用或切换凭据
            // （避免 502 high load 等瞬态错误把所有凭据锁死）
            if matches!(status.as_u16(), 408) || status.is_server_error() {
                tracing::warn!(
                    "API 请求失败（上游瞬态错误，尝试 {}/{}）: {} {}",
                    attempt + 1,
                    max_retries,
                    status,
                    body
                );
                self.token_manager.record_api_fail(ctx.id);

                if Self::is_model_temporarily_unavailable(&body) {
                    tracing::warn!(
                        credential_id = %ctx.id,
                        "{} API 5xx 遇到 MODEL_TEMPORARILY_UNAVAILABLE（上游过载），按瞬态错误重试",
                        api_type
                    );
                }

                last_error = Some(anyhow::anyhow!(
                    "{} API 请求失败: {} {}",
                    api_type,
                    status,
                    body
                ));
                // P0#1：5xx 瞬态错误也 push，避免连续撞同一个上游路径
                failed_ids.push(ctx.id);
                if attempt + 1 < max_retries {
                    sleep(Self::retry_delay(attempt)).await;
                }
                continue;
            }

            // 其他 4xx - 通常为请求/配置问题：直接返回，不计入凭据失败
            if status.is_client_error() {
                anyhow::bail!("{} API 请求失败: {} {}", api_type, status, body);
            }

            // 兜底：当作可重试的瞬态错误处理
            tracing::warn!(
                "API 请求失败（未知错误，尝试 {}/{}）: {} {}",
                attempt + 1,
                max_retries,
                status,
                body
            );
            last_error = Some(anyhow::anyhow!(
                "{} API 请求失败: {} {}",
                api_type,
                status,
                body
            ));
            // P0#1：兜底也切换凭据，避免未知错误反复撞同一个
            failed_ids.push(ctx.id);
            if attempt + 1 < max_retries {
                sleep(Self::retry_delay(attempt)).await;
            }
        }

        // 所有重试都失败
        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!(
                "{} API 请求失败：已达到最大重试次数（{}次）",
                api_type,
                max_retries
            )
        }))
    }

    fn retry_delay(attempt: usize) -> Duration {
        const BASE_MS: u64 = 200;
        const MAX_MS: u64 = 2_000;
        let exp = BASE_MS.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
        let backoff = exp.min(MAX_MS);
        let jitter_max = (backoff / 4).max(1);
        let jitter = fastrand::u64(0..=jitter_max);
        Duration::from_millis(backoff.saturating_add(jitter))
    }

    fn retry_delay_throttle(attempt: usize) -> Duration {
        const BASE_MS: u64 = 1_000;
        const MAX_MS: u64 = 8_000;
        let exp = BASE_MS.saturating_mul(2u64.saturating_pow(attempt.min(6) as u32));
        let backoff = exp.min(MAX_MS);
        let jitter_max = (backoff / 4).max(1);
        let jitter = fastrand::u64(0..=jitter_max);
        Duration::from_millis(backoff.saturating_add(jitter))
    }

    /// 把当前凭据放入 RateLimitExceeded 冷却（trigger_count 指数退避）。
    ///
    /// **Round 8 决议后保留但默认不调用** —— 平台不再在 429 路径冻凭据
    /// （详见 4 个 429 分支的注释）。函数保留以便：
    ///   a) 未来如果需要按 model 或按 reason 选择性冻凭据，可重新接入
    ///   b) token_manager.set_credential_cooldown_with_duration 仍由 401/403
    ///      / TokenRefreshFailed 等"凭据真坏"路径使用
    #[allow(dead_code)]
    fn handle_rate_limited_response(
        &self,
        credential_id: u64,
        body: &str,
        retry_after: Option<Duration>,
    ) -> Duration {
        let cooldown = self.token_manager.set_credential_cooldown_with_duration(
            credential_id,
            crate::kiro::cooldown::CooldownReason::RateLimitExceeded,
            retry_after,
        );

        tracing::warn!(
            credential_id = %credential_id,
            retry_after_secs = ?retry_after.map(|d| d.as_secs()),
            cooldown_secs = %cooldown.as_secs(),
            rate_limit_response = %Self::is_rate_limit_response(body),
            "凭据触发 429 限流，已设置冷却"
        );

        cooldown
    }

    fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
        let raw = headers.get("retry-after")?.to_str().ok()?.trim();
        if raw.is_empty() {
            return None;
        }

        if let Ok(seconds) = raw.parse::<u64>() {
            return Some(Self::clamp_rate_limit_cooldown(Duration::from_secs(
                seconds,
            )));
        }

        let retry_at = DateTime::parse_from_rfc2822(raw).ok()?.with_timezone(&Utc);
        let now = Utc::now();
        let wait = retry_at.signed_duration_since(now).to_std().ok()?;
        Some(Self::clamp_rate_limit_cooldown(wait))
    }

    fn clamp_rate_limit_cooldown(duration: Duration) -> Duration {
        duration.clamp(
            Duration::from_secs(MIN_RATE_LIMIT_COOLDOWN_SECS),
            Duration::from_secs(MAX_RATE_LIMIT_COOLDOWN_SECS),
        )
    }

    /// 检测响应体是否为 rate-limit 类错误。Round 8 决议后默认不接入，参见
    /// `handle_rate_limited_response` 的 `#[allow(dead_code)]` 注释。
    #[allow(dead_code)]
    fn is_rate_limit_response(body: &str) -> bool {
        let lower = body.to_ascii_lowercase();
        if lower.contains("rate limit")
            || lower.contains("too many requests")
            || lower.contains("high traffic")
            || lower.contains("request limit")
        {
            return true;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return false;
        };

        let reason_matches = |s: &str| {
            let upper = s.to_ascii_uppercase();
            upper.contains("RATE_LIMIT")
                || upper.contains("TOO_MANY_REQUESTS")
                || upper.contains("REQUEST_LIMIT")
                || upper.contains("HIGH_TRAFFIC")
        };

        value
            .get("reason")
            .and_then(|v| v.as_str())
            .is_some_and(reason_matches)
            || value
                .pointer("/error/reason")
                .and_then(|v| v.as_str())
                .is_some_and(reason_matches)
    }

    /// 检测是否为 MODEL_TEMPORARILY_UNAVAILABLE 错误
    fn is_model_temporarily_unavailable(body: &str) -> bool {
        if body.contains("MODEL_TEMPORARILY_UNAVAILABLE") {
            return true;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
            return false;
        };

        if value
            .get("reason")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "MODEL_TEMPORARILY_UNAVAILABLE")
        {
            return true;
        }

        value
            .pointer("/error/reason")
            .and_then(|v| v.as_str())
            .is_some_and(|v| v == "MODEL_TEMPORARILY_UNAVAILABLE")
    }

    /// 检测是否为「输入过长」类错误
    ///
    /// 典型返回：
    /// `{"message":"Input is too long.","reason":"CONTENT_LENGTH_EXCEEDS_THRESHOLD"}`
    fn is_input_too_long(body: &str) -> bool {
        body.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") || body.contains("Input is too long")
    }

    /// 从上游响应体提取一个适合返回给客户端的错误摘要
    ///
    /// 目标：
    /// - 保留关键错误信息（例如 "Input is too long" / "Improperly formed request"）
    /// - 避免返回过长/不可控的内容导致客户端难以区分或处理
    #[cfg(not(feature = "sensitive-logs"))]
    fn summarize_error_body(body: &str) -> String {
        const MAX_LEN: usize = 256;
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return "<empty response body>".to_string();
        }

        // 优先尝试解析 JSON，从常见字段中提取 message / reason。
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("Message").and_then(|v| v.as_str()))
                .or_else(|| value.pointer("/error/message").and_then(|v| v.as_str()))
                .or_else(|| value.pointer("/error/Message").and_then(|v| v.as_str()));

            let reason = value
                .get("reason")
                .and_then(|v| v.as_str())
                .or_else(|| value.get("Reason").and_then(|v| v.as_str()))
                .or_else(|| value.pointer("/error/reason").and_then(|v| v.as_str()))
                .or_else(|| value.pointer("/error/Reason").and_then(|v| v.as_str()));

            if let Some(msg) = message {
                let mut s = msg.to_string();
                if let Some(r) = reason.filter(|r| !r.is_empty() && *r != "null") {
                    // 避免重复拼接（有些上游会把 reason 直接写入 message）
                    if !msg.contains(r) {
                        s.push_str(&format!(" (reason={})", r));
                    }
                }
                return Self::truncate_one_line(&s, MAX_LEN);
            }
        }

        // JSON 解析失败或不含常见字段，回退到压缩后的纯文本。
        Self::truncate_one_line(trimmed, MAX_LEN)
    }

    #[cfg(not(feature = "sensitive-logs"))]
    fn truncate_one_line(s: &str, max_len: usize) -> String {
        let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if one_line.len() <= max_len {
            return one_line;
        }

        let end = floor_char_boundary(&one_line, max_len);
        format!("{}...", &one_line[..end])
    }

    /// 估算文本的 token 数量
    ///
    /// 基于字符类型的估算公式：
    /// - CJK 字符（中/日/韩）: token 数 = 字符数 / 1.5
    /// - 其他字符（英文等）: token 数 = 字符数 / 3.5
    fn estimate_tokens(text: &str) -> usize {
        let mut cjk_count = 0usize;
        let mut other_count = 0usize;

        for c in text.chars() {
            if Self::is_cjk_char(c) {
                cjk_count += 1;
            } else {
                other_count += 1;
            }
        }

        let cjk_tokens = cjk_count as f64 / 1.5;
        let other_tokens = other_count as f64 / 3.5;
        (cjk_tokens + other_tokens + 0.5) as usize
    }

    /// 判断是否为 CJK（中日韩）字符
    #[inline]
    fn is_cjk_char(c: char) -> bool {
        matches!(c,
            '\u{4E00}'..='\u{9FFF}'   |  // CJK 统一汉字
            '\u{3400}'..='\u{4DBF}'   |  // CJK 扩展 A
            '\u{20000}'..='\u{2A6DF}' |  // CJK 扩展 B
            '\u{2A700}'..='\u{2B73F}' |  // CJK 扩展 C
            '\u{2B740}'..='\u{2B81F}' |  // CJK 扩展 D
            '\u{F900}'..='\u{FAFF}'   |  // CJK 兼容汉字
            '\u{2F800}'..='\u{2FA1F}' |  // CJK 兼容扩展
            '\u{3000}'..='\u{303F}'   |  // CJK 标点符号
            '\u{3040}'..='\u{309F}'   |  // 平假名
            '\u{30A0}'..='\u{30FF}'   |  // 片假名
            '\u{AC00}'..='\u{D7AF}'      // 韩文音节
        )
    }

    /// 截断请求体用于日志输出，保留头尾各 `keep` 个字符
    ///
    /// Debug-only wire capture helper. The destination directory is read from
    /// `KIRO_RS_CAPTURE` exactly once per process and cached, so the hot path
    /// pays at most one OnceLock load + an `Option::is_some` check when the
    /// env var is unset.
    #[cfg(debug_assertions)]
    fn wire_capture(body: &str) {
        use std::sync::OnceLock;
        static DIR: OnceLock<Option<String>> = OnceLock::new();
        let Some(dir) = DIR.get_or_init(|| std::env::var("KIRO_RS_CAPTURE").ok()) else {
            return;
        };
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!(target: "kiro_rs_capture", "create_dir_all({dir}): {e}");
            return;
        }
        let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S-%3f").to_string();
        let path = format!("{dir}/gar-{ts}.json");
        if let Err(e) = std::fs::write(&path, body) {
            tracing::warn!(target: "kiro_rs_capture", "write({path}): {e}");
            return;
        }
        tracing::info!(target: "kiro_rs_capture", "wrote {path} ({} bytes)", body.len());
    }

    /// 避免在 sensitive-logs 模式下输出包含大量 base64 图片数据的完整请求体。
    #[cfg(feature = "sensitive-logs")]
    fn truncate_body_for_log(s: &str, keep: usize) -> std::borrow::Cow<'_, str> {
        let char_count = s.chars().count();
        let min_omit = 30;
        if char_count <= keep * 2 + min_omit {
            return std::borrow::Cow::Borrowed(s);
        }

        let head_end = s
            .char_indices()
            .nth(keep)
            .map(|(i, _)| i)
            .unwrap_or(s.len());

        let tail_start = s
            .char_indices()
            .nth_back(keep - 1)
            .map(|(i, _)| i)
            .unwrap_or(0);

        let omitted = s.len() - head_end - (s.len() - tail_start);
        std::borrow::Cow::Owned(format!(
            "{}...({} bytes omitted)...{}",
            &s[..head_end],
            omitted,
            &s[tail_start..]
        ))
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::kiro::cooldown::CooldownReason;
    use crate::kiro::endpoint::{
        CliEndpoint, IdeEndpoint, default_is_bearer_token_invalid, default_is_monthly_request_limit,
    };
    use crate::kiro::model::credentials::{
        BUILDER_ID_PROFILE_ARN, KiroCredentials, SOCIAL_PROFILE_ARN,
    };
    use crate::kiro::model::events::Event;
    use crate::kiro::parser::frame::Frame;
    use crate::kiro::parser::header::{HeaderValue as EventHeaderValue, Headers};
    use crate::model::config::Config;
    use reqwest::header::{AUTHORIZATION, CONNECTION, CONTENT_TYPE, HeaderValue};

    fn create_test_provider(config: Config, credentials: KiroCredentials) -> KiroProvider {
        let tm = MultiTokenManager::new(config, vec![credentials], None, None, false).unwrap();
        KiroProvider::new(Arc::new(tm))
    }

    #[test]
    fn test_strip_reasoning_content_for_retry_removes_only_history_assistant_reasoning() {
        let body = serde_json::json!({
            "conversationState": {
                "history": [
                    {
                        "assistantResponseMessage": {
                            "content": "answer",
                            "reasoningContent": { "text": "stale", "signature": "bad" },
                            "toolUses": []
                        }
                    },
                    {
                        "userInputMessage": {
                            "content": "next",
                            "reasoningContent": "must stay"
                        }
                    }
                ],
                "currentMessage": {
                    "userInputMessage": { "content": "hello" }
                }
            },
            "additionalModelRequestFields": {
                "thinking": { "type": "adaptive" }
            }
        })
        .to_string();

        let stripped = KiroProvider::strip_reasoning_content_for_retry(&body).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&stripped).unwrap();

        assert!(
            parsed["conversationState"]["history"][0]["assistantResponseMessage"]
                .get("reasoningContent")
                .is_none()
        );
        assert_eq!(
            parsed["conversationState"]["history"][1]["userInputMessage"]["reasoningContent"],
            "must stay"
        );
        assert_eq!(
            parsed["additionalModelRequestFields"]["thinking"]["type"],
            "adaptive"
        );
    }

    #[test]
    fn test_extract_model_id_from_kiro_request_body() {
        let body = serde_json::json!({
            "conversationState": {
                "currentMessage": {
                    "userInputMessage": {
                        "content": "hello",
                        "modelId": "claude-opus-4.6"
                    }
                }
            }
        })
        .to_string();

        assert_eq!(
            KiroProvider::extract_model_id(&body).as_deref(),
            Some("claude-opus-4.6")
        );
    }

    #[test]
    fn test_is_suspended_signal_case_insensitive() {
        assert!(is_suspended_signal("suspended"));
        assert!(is_suspended_signal("Account Suspended"));
        assert!(is_suspended_signal("ACCOUNT_SUSPENDED"));
        assert!(is_suspended_signal("error: TEMPORARILY_SUSPENDED"));
        assert!(!is_suspended_signal("rate limit exceeded"));
        assert!(!is_suspended_signal(""));
    }

    #[test]
    fn test_cli_endpoint_api_url() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        assert!(endpoint.api_url(&ctx).contains("amazonaws.com"));
        assert!(endpoint.api_url(&ctx).contains("generateAssistantResponse"));
    }

    #[test]
    fn test_cli_endpoint_decorate_api_headers() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();

        let credentials = KiroCredentials::default();
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let request = endpoint.decorate_api(
            reqwest::Client::new()
                .post("https://example.com")
                .header("Connection", "close"),
            &ctx,
        );
        let built = request.build().unwrap();

        // Byte-aligned with kiro-cli 2.3.0 capture 2026-05-12.
        assert_eq!(
            built.headers().get("x-amz-target").unwrap(),
            "AmazonCodeWhispererStreamingService.GenerateAssistantResponse"
        );
        assert_eq!(
            built.headers().get(CONTENT_TYPE).unwrap(),
            "application/x-amz-json-1.0"
        );
        assert_eq!(
            built.headers().get("x-amzn-codewhisperer-optout").unwrap(),
            "false"
        );
        // kiro-cli does NOT send `x-amzn-kiro-agent-mode` on this endpoint.
        assert!(
            built.headers().get("x-amzn-kiro-agent-mode").is_none(),
            "x-amzn-kiro-agent-mode is IDE-only; kiro-cli does not send it"
        );
        assert_eq!(built.headers().get(CONNECTION).unwrap(), "close");
    }

    #[test]
    fn test_cli_endpoint_transform_api_body_rewrites_origin() {
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let request_body = r#"{"conversationState":{"currentMessage":{"userInputMessage":{"origin":"AI_EDITOR"}},"history":[{"userInputMessage":{"origin":"AI_EDITOR"}}]}}"#;
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["conversationState"]["currentMessage"]["userInputMessage"]["origin"],
            "KIRO_CLI"
        );
        assert_eq!(
            parsed["conversationState"]["history"][0]["userInputMessage"]["origin"],
            "KIRO_CLI"
        );
    }

    /// kiro-cli 2.3.0 wire byte-alignment: confirm transform_api_body emits the
    /// CONTEXT-ENTRY-wrapped currentMessage.content + envState + auto modelId
    /// in the exact field order observed in docs/golden-gar-body.json.
    #[test]
    fn test_cli_endpoint_transform_api_body_matches_golden_shape() {
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        // Minimal body shape produced by converter (struct declaration order
        // matches kiro-cli wire; ctx is empty so it serializes as `{}`).
        let request_body = serde_json::json!({
            "conversationState": {
                "conversationId": "conv-1",
                "history": [
                    {"userInputMessage": {
                        "content": "h0",
                        "origin": "AI_EDITOR",
                        "modelId": "claude-sonnet-4-20250514"
                    }},
                    {"assistantResponseMessage": {"content": "ack"}}
                ],
                "currentMessage": {"userInputMessage": {
                    "content": "hello",
                    "userInputMessageContext": {},
                    "origin": "AI_EDITOR",
                    "modelId": "claude-sonnet-4-20250514"
                }},
                "chatTriggerType": "MANUAL",
                "agentContinuationId": "cont-1",
                "agentTaskType": "vibe"
            },
            "profileArn": "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK"
        });
        let result = endpoint
            .transform_api_body(&serde_json::to_string(&request_body).unwrap(), &ctx)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let cs = &parsed["conversationState"];
        let cur_uim = &cs["currentMessage"]["userInputMessage"];

        // modelId is forced to "auto" on the CLI endpoint.
        assert_eq!(cur_uim["modelId"], "auto");
        assert_eq!(cs["history"][0]["userInputMessage"]["modelId"], "auto");

        // origin → KIRO_CLI everywhere.
        assert_eq!(cur_uim["origin"], "KIRO_CLI");

        // currentMessage.content wrapped with CONTEXT ENTRY / USER MESSAGE
        // markers + a Current time line.
        let content = cur_uim["content"].as_str().unwrap();
        assert!(content.contains("--- CONTEXT ENTRY BEGIN ---"));
        assert!(content.contains("Current time:"));
        assert!(content.contains("--- CONTEXT ENTRY END ---"));
        assert!(content.contains("--- USER MESSAGE BEGIN ---\nhello--- USER MESSAGE END ---"));

        // envState injected on currentMessage and on every history user turn.
        let cur_ctx = &cur_uim["userInputMessageContext"];
        assert!(
            !cur_ctx["envState"]["operatingSystem"]
                .as_str()
                .unwrap()
                .is_empty()
        );
        assert!(
            cur_ctx["envState"]["currentWorkingDirectory"]
                .as_str()
                .is_some()
        );

        let h0_ctx = &cs["history"][0]["userInputMessage"]["userInputMessageContext"];
        assert!(h0_ctx["envState"].is_object());

        // Idempotency: running transform_api_body twice doesn't double-wrap.
        let second = endpoint.transform_api_body(&result, &ctx).unwrap();
        let second_parsed: serde_json::Value = serde_json::from_str(&second).unwrap();
        let second_content =
            second_parsed["conversationState"]["currentMessage"]["userInputMessage"]["content"]
                .as_str()
                .unwrap();
        assert_eq!(
            second_content.matches("--- USER MESSAGE BEGIN ---").count(),
            1,
            "transform must be idempotent — markers should not stack on retry"
        );
    }

    /// Verifies that struct declaration order in conversation.rs matches the
    /// kiro-cli wire (after preserve_order feature on serde_json reads body
    /// without alphabetizing).
    #[test]
    fn test_cli_endpoint_preserves_field_order_through_transform() {
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        // Build via the typed converter structs so field order is owned by
        // Rust struct declaration (the path real requests take).
        use crate::kiro::model::requests::conversation::{
            ConversationState, CurrentMessage, Message, UserInputMessage,
        };
        let cur = CurrentMessage::new(
            UserInputMessage::new("hi", "claude-sonnet-4").with_origin("AI_EDITOR"),
        );
        let state = ConversationState::new("c1")
            .with_history(vec![
                Message::user("h", "claude-sonnet-4"),
                Message::assistant("ack"),
            ])
            .with_current_message(cur)
            .with_chat_trigger_type("MANUAL")
            .with_agent_continuation_id("ac1")
            .with_agent_task_type("vibe");
        let body = serde_json::json!({"conversationState": state, "profileArn": "arn:x"});
        let result = endpoint
            .transform_api_body(&serde_json::to_string(&body).unwrap(), &ctx)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let cs = &parsed["conversationState"];
        let keys: Vec<&str> = cs.as_object().unwrap().keys().map(|s| s.as_str()).collect();
        // Golden order (kiro-cli 2.3.0): conversationId, history, currentMessage,
        // chatTriggerType, agentContinuationId, agentTaskType.
        assert_eq!(
            keys,
            vec![
                "conversationId",
                "history",
                "currentMessage",
                "chatTriggerType",
                "agentContinuationId",
                "agentTaskType",
            ],
            "conversationState field order must match kiro-cli golden capture"
        );
        let cur_keys: Vec<&str> = cs["currentMessage"]["userInputMessage"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            cur_keys,
            vec!["content", "userInputMessageContext", "origin", "modelId"],
            "currentMessage.userInputMessage field order must match golden"
        );
    }

    /// Round 4 regression: user-controlled tool inputSchema must NOT be touched
    /// by rewrite_origin_and_model. Pre-fix the recursion would clobber any
    /// schema property named "origin" or "modelId" with CLI canonical values.
    #[test]
    fn test_cli_endpoint_does_not_rewrite_user_tool_input_schema() {
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let body = serde_json::json!({
            "conversationState": {
                "conversationId": "c1",
                "currentMessage": {"userInputMessage": {
                    "content": "hi",
                    "userInputMessageContext": {
                        "tools": [{
                            "toolSpecification": {
                                "inputSchema": {
                                    "json": {
                                        "type": "object",
                                        "properties": {
                                            "origin": {"type": "string", "description": "should survive"},
                                            "modelId": {"type": "string", "description": "should survive"}
                                        }
                                    }
                                },
                                "name": "tool",
                                "description": "test"
                            }
                        }]
                    },
                    "origin": "AI_EDITOR",
                    "modelId": "claude-sonnet-4-20250514"
                }},
                "chatTriggerType": "MANUAL",
                "agentTaskType": "vibe"
            },
            "profileArn": "arn:x"
        });
        let result = endpoint
            .transform_api_body(&serde_json::to_string(&body).unwrap(), &ctx)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        let props = &parsed["conversationState"]["currentMessage"]["userInputMessage"]["userInputMessageContext"]
            ["tools"][0]["toolSpecification"]["inputSchema"]["json"]["properties"];
        // User-defined schema properties must be preserved verbatim.
        assert_eq!(
            props["origin"],
            serde_json::json!({"type": "string", "description": "should survive"})
        );
        assert_eq!(
            props["modelId"],
            serde_json::json!({"type": "string", "description": "should survive"})
        );
        // But the protocol fields ARE rewritten:
        let cur_uim = &parsed["conversationState"]["currentMessage"]["userInputMessage"];
        assert_eq!(cur_uim["origin"], "KIRO_CLI");
        assert_eq!(cur_uim["modelId"], "auto");
    }

    /// Round 4 regression: CLI endpoint must inject the credential's profileArn
    /// per-request (previously the field came from a static state snapshot of
    /// only the FIRST credential — multi-credential rotation broke).
    #[test]
    fn test_cli_endpoint_injects_credentials_profile_arn() {
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let mut credentials = KiroCredentials::default();
        credentials.profile_arn = Some("arn:per-request-correct".to_string());
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let body = serde_json::json!({
            "conversationState": {
                "conversationId": "c1",
                "currentMessage": {"userInputMessage": {"content": "hi"}},
            },
            "profileArn": "arn:stale-startup-snapshot"
        });
        let result = endpoint
            .transform_api_body(&serde_json::to_string(&body).unwrap(), &ctx)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["profileArn"], "arn:per-request-correct");
    }

    /// Round 4 regression: IDC / Builder-ID credentials must NOT send profileArn.
    #[test]
    fn test_cli_endpoint_injects_profile_arn_for_sso_oidc() {
        let endpoint = CliEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("idc".to_string());
        credentials.profile_arn = Some("arn:real-idc-profile".to_string());
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let body = serde_json::json!({
            "conversationState": {"conversationId": "c1", "currentMessage": {"userInputMessage": {"content": "hi"}}},
            "profileArn": "arn:stale-old-value"
        });
        let result = endpoint
            .transform_api_body(&serde_json::to_string(&body).unwrap(), &ctx)
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["profileArn"].as_str().unwrap(),
            "arn:real-idc-profile",
            "IdC credentials must inject their own profileArn"
        );
    }

    #[test]
    fn test_ide_endpoint_api_url() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        assert!(endpoint.api_url(&ctx).contains("amazonaws.com"));
        assert!(endpoint.api_url(&ctx).contains("generateAssistantResponse"));
    }

    #[test]
    fn test_ide_endpoint_host_like_domain() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        let credentials = KiroCredentials::default();
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let request =
            endpoint.decorate_api(reqwest::Client::new().post("https://example.com"), &ctx);
        let built = request.build().unwrap();
        assert_eq!(
            built.headers().get("host").unwrap(),
            "q.us-east-1.amazonaws.com"
        );
    }

    #[test]
    fn test_ide_runtime_endpoint_host_like_domain() {
        let mut config = Config::default();
        config.region = "eu-central-1".to_string();
        let credentials = KiroCredentials::default();
        let endpoint = IdeEndpoint::runtime();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };

        assert_eq!(
            endpoint.api_url(&ctx),
            "https://runtime.eu-central-1.kiro.dev/generateAssistantResponse"
        );

        let request =
            endpoint.decorate_api(reqwest::Client::new().post("https://example.com"), &ctx);
        let built = request.build().unwrap();
        assert_eq!(
            built.headers().get("host").unwrap(),
            "runtime.eu-central-1.kiro.dev"
        );
    }

    #[test]
    fn test_ide_endpoint_decorate_api_headers() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.8.0".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.profile_arn = Some("arn:aws:sso::123456789:profile/test".to_string());
        credentials.refresh_token = Some("a".repeat(150));

        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let request = endpoint.decorate_api(
            reqwest::Client::new()
                .post("https://example.com")
                .header("Connection", "close"),
            &ctx,
        );
        let built = request.build().unwrap();

        assert_eq!(
            built.headers().get(CONTENT_TYPE).unwrap(),
            "application/json"
        );
        assert_eq!(
            built.headers().get("x-amzn-codewhisperer-optout").unwrap(),
            "true"
        );
        assert_eq!(
            built.headers().get("x-amzn-kiro-agent-mode").unwrap(),
            "vibe"
        );
        assert!(
            built
                .headers()
                .get(AUTHORIZATION)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Bearer ")
        );
        assert_eq!(built.headers().get(CONNECTION).unwrap(), "close");
    }

    #[test]
    fn test_ide_runtime_endpoint_decorate_api_headers() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.11.107".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.profile_arn = Some("arn:aws:sso::123456789:profile/test".to_string());
        credentials.refresh_token = Some("a".repeat(150));

        let endpoint = IdeEndpoint::runtime();
        let machine_id = "b".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let request = endpoint.decorate_api(
            reqwest::Client::new()
                .post("https://example.com")
                .header("Connection", "close"),
            &ctx,
        );
        let built = request.build().unwrap();

        assert_eq!(
            built.headers().get("host").unwrap(),
            "runtime.us-east-1.kiro.dev"
        );
        assert_eq!(
            built.headers().get(CONTENT_TYPE).unwrap(),
            "application/x-amz-json-1.0"
        );
        assert_eq!(
            built.headers().get("x-amz-target").unwrap(),
            "AmazonCodeWhispererStreamingService.GenerateAssistantResponse"
        );
        assert_eq!(
            built.headers().get("x-amzn-codewhisperer-optout").unwrap(),
            "true"
        );
        assert_eq!(
            built.headers().get("x-amzn-kiro-agent-mode").unwrap(),
            "vibe"
        );
        assert!(
            built
                .headers()
                .get("x-amz-user-agent")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("aws-sdk-js/1.0.27 KiroIDE-0.7.45-")
        );
        assert!(
            built
                .headers()
                .get("user-agent")
                .unwrap()
                .to_str()
                .unwrap()
                .contains("api/codewhispererstreaming#1.0.27")
        );
        assert!(
            built
                .headers()
                .get(AUTHORIZATION)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("Bearer ")
        );
        assert_eq!(built.headers().get(CONNECTION).unwrap(), "close");
    }

    #[test]
    fn test_ide_endpoint_decorate_api_sets_tokentype() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.8.0".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("api_key".to_string());
        credentials.kiro_api_key = Some("ksk_test_api_key".to_string());
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "ksk_test_api_key",
            machine_id: &machine_id,
            config: &config,
        };
        let request =
            endpoint.decorate_api(reqwest::Client::new().post("https://example.com"), &ctx);
        let built = request.build().unwrap();
        assert_eq!(built.headers().get("tokentype").unwrap(), "API_KEY");
    }

    #[test]
    fn test_ide_endpoint_decorate_mcp_includes_profile_arn_for_social_auth() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.8.0".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("social".to_string());
        credentials.profile_arn = Some("arn:aws:sso::123456789:profile/test".to_string());
        credentials.refresh_token = Some("a".repeat(150));
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let request =
            endpoint.decorate_mcp(reqwest::Client::new().post("https://example.com"), &ctx);
        let built = request.build().unwrap();
        assert_eq!(
            built
                .headers()
                .get("x-amzn-kiro-profile-arn")
                .unwrap()
                .to_str()
                .unwrap(),
            "arn:aws:sso::123456789:profile/test"
        );
    }

    #[test]
    fn test_ide_endpoint_decorate_mcp_includes_real_profile_arn_for_idc_auth() {
        let mut config = Config::default();
        config.region = "us-east-1".to_string();
        config.kiro_version = "0.8.0".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("idc".to_string());
        credentials.profile_arn = Some("arn:aws:sso::123456789:profile/test".to_string());
        credentials.client_id = Some("client".to_string());
        credentials.client_secret = Some("secret".to_string());
        credentials.refresh_token = Some("a".repeat(150));
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let request =
            endpoint.decorate_mcp(reqwest::Client::new().post("https://example.com"), &ctx);
        let built = request.build().unwrap();
        assert_eq!(
            built
                .headers()
                .get("x-amzn-kiro-profile-arn")
                .unwrap()
                .to_str()
                .unwrap(),
            "arn:aws:sso::123456789:profile/test"
        );
    }

    #[test]
    fn test_ide_runtime_endpoint_injects_profile_arn_for_social_auth() {
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("social".to_string());
        credentials.profile_arn = Some("arn:aws:sso::111111111:profile/social-profile".to_string());

        let request_body = r#"{"conversationState":{"conversationId":"test"}}"#;
        let endpoint = IdeEndpoint::runtime();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["profileArn"].as_str().unwrap(),
            "arn:aws:sso::111111111:profile/social-profile"
        );
        assert_eq!(parsed["conversationState"]["conversationId"], "test");
    }

    #[test]
    fn test_ide_runtime_endpoint_does_not_inject_profile_arn_for_api_key() {
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("api_key".to_string());
        credentials.kiro_api_key = Some("ksk_test_api_key".to_string());

        let request_body = r#"{"conversationState":{"conversationId":"test"}}"#;
        let endpoint = IdeEndpoint::runtime();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "ksk_test_api_key",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(
            parsed.get("profileArn").is_none(),
            "API key runtime requests must not inject profileArn"
        );
        assert_eq!(parsed["conversationState"]["conversationId"], "test");
    }

    #[test]
    fn test_ide_runtime_streaming_parser_accepts_usage_frames() {
        fn event_frame(event_type: &str, payload: &'static [u8]) -> Frame {
            let mut headers = Headers::new();
            headers.insert(
                ":message-type".to_string(),
                EventHeaderValue::String("event".to_string()),
            );
            headers.insert(
                ":event-type".to_string(),
                EventHeaderValue::String(event_type.to_string()),
            );
            Frame {
                headers,
                payload: payload.to_vec(),
            }
        }

        let token_usage = Event::from_frame(event_frame(
            "tokenUsageEvent",
            br#"{"uncachedInputTokens":100,"outputTokens":20,"totalTokens":120,"cacheReadInputTokens":10,"cacheWriteInputTokens":5}"#,
        ))
        .unwrap();
        match token_usage {
            Event::TokenUsage(usage) => {
                assert_eq!(usage.uncached_input_tokens, 100);
                assert_eq!(usage.output_tokens, 20);
                assert_eq!(usage.total_tokens, 120);
                assert_eq!(usage.cache_read_input_tokens, Some(10));
                assert_eq!(usage.cache_write_input_tokens, Some(5));
            }
            other => panic!("expected tokenUsageEvent, got {other:?}"),
        }

        let metering = Event::from_frame(event_frame(
            "meteringEvent",
            br#"{"unit":"credit","unitPlural":"credits","usage":0.25}"#,
        ))
        .unwrap();
        match metering {
            Event::Metering(metering) => {
                assert_eq!(metering.unit, "credit");
                assert_eq!(metering.unit_plural, "credits");
                assert_eq!(metering.usage, 0.25);
            }
            other => panic!("expected meteringEvent, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_retry_after_seconds() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("120"));

        let wait = KiroProvider::parse_retry_after(&headers).unwrap();
        assert_eq!(wait, Duration::from_secs(120));
    }

    #[test]
    fn test_parse_retry_after_http_date() {
        let mut headers = HeaderMap::new();
        let future = (Utc::now() + chrono::Duration::seconds(90)).to_rfc2822();
        headers.insert("retry-after", HeaderValue::from_str(&future).unwrap());

        let wait = KiroProvider::parse_retry_after(&headers).unwrap();
        assert!(wait >= Duration::from_secs(5));
        assert!(wait <= Duration::from_secs(120));
    }

    #[test]
    fn test_parse_retry_after_invalid() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("not-a-date"));

        assert!(KiroProvider::parse_retry_after(&headers).is_none());
    }

    #[test]
    fn test_parse_retry_after_clamps_range() {
        let mut headers = HeaderMap::new();
        // 下限 5s
        headers.insert("retry-after", HeaderValue::from_static("2"));
        assert_eq!(
            KiroProvider::parse_retry_after(&headers).unwrap(),
            Duration::from_secs(5)
        );

        // 上限 120s
        headers.insert("retry-after", HeaderValue::from_static("600"));
        assert_eq!(
            KiroProvider::parse_retry_after(&headers).unwrap(),
            Duration::from_secs(120)
        );

        // 正常值透传
        headers.insert("retry-after", HeaderValue::from_static("15"));
        assert_eq!(
            KiroProvider::parse_retry_after(&headers).unwrap(),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn test_is_rate_limit_response_detects_reason() {
        let body = r#"{"message":"Too many requests","reason":"RATE_LIMIT_EXCEEDED"}"#;
        assert!(KiroProvider::is_rate_limit_response(body));
    }

    #[test]
    fn test_is_rate_limit_response_detects_nested_reason() {
        let body = r#"{"error":{"reason":"REQUEST_LIMIT_5_MINUTES"}}"#;
        assert!(KiroProvider::is_rate_limit_response(body));
    }

    #[test]
    fn test_is_rate_limit_response_false() {
        let body = r#"{"message":"Forbidden","reason":"AUTH_FAILED"}"#;
        assert!(!KiroProvider::is_rate_limit_response(body));
    }

    #[test]
    fn test_handle_rate_limited_response_sets_cooldown() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let provider = create_test_provider(config, credentials);
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("120"));

        let cooldown = provider.handle_rate_limited_response(
            1,
            "Too many requests",
            KiroProvider::parse_retry_after(&headers),
        );
        assert_eq!(cooldown, Duration::from_secs(120));

        let (reason, remaining) = provider
            .token_manager()
            .cooldown_manager()
            .check_cooldown(1)
            .unwrap();
        assert_eq!(reason, CooldownReason::RateLimitExceeded);
        assert!(remaining <= Duration::from_secs(120));
        assert!(remaining > Duration::from_secs(100));

        let snapshot = provider.token_manager().snapshot();
        assert_eq!(snapshot.entries[0].failure_count, 0);
        assert!(!snapshot.entries[0].disabled);
        assert!(snapshot.entries[0].last_used_at.is_some());
    }

    #[test]
    fn test_handle_rate_limited_response_without_retry_after_uses_default_cooldown() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let provider = create_test_provider(config, credentials);

        // handle_rate_limited_response 传 None → CooldownManager 用 default_duration() = 60s
        // 注意：此函数为 dead code（Round 8 后不再调用），实际 429 路径直接用 DEFAULT_RATE_LIMIT_COOLDOWN_SECS
        let cooldown = provider.handle_rate_limited_response(1, "Too many requests", None);
        assert_eq!(cooldown, Duration::from_secs(60));

        let (reason, remaining) = provider
            .token_manager()
            .cooldown_manager()
            .check_cooldown(1)
            .unwrap();
        assert_eq!(reason, CooldownReason::RateLimitExceeded);
        assert!(remaining <= Duration::from_secs(60));
        assert!(remaining > Duration::from_secs(50));
    }

    #[test]
    fn test_is_monthly_request_limit_detects_reason() {
        let body = r#"{"message":"You have reached the limit.","reason":"MONTHLY_REQUEST_COUNT"}"#;
        assert!(default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_is_monthly_request_limit_nested_reason() {
        let body = r#"{"error":{"reason":"MONTHLY_REQUEST_COUNT"}}"#;
        assert!(default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_is_monthly_request_limit_false() {
        let body = r#"{"message":"nope","reason":"DAILY_REQUEST_COUNT"}"#;
        assert!(!default_is_monthly_request_limit(body));
    }

    #[test]
    fn test_is_invalid_bearer_token_true() {
        let body =
            r#"{"message":"The bearer token included in the request is invalid.","reason":null}"#;
        assert!(default_is_bearer_token_invalid(body));
    }

    #[test]
    fn test_is_invalid_bearer_token_false() {
        let body = r#"{"message":"Forbidden","reason":null}"#;
        assert!(!default_is_bearer_token_invalid(body));
    }

    #[test]
    #[cfg(not(feature = "sensitive-logs"))]
    fn test_summarize_error_body_extracts_message_and_reason() {
        let body =
            r#"{"message":"Input is too long.","reason":"CONTENT_LENGTH_EXCEEDS_THRESHOLD"}"#;
        let summary = KiroProvider::summarize_error_body(body);
        assert!(summary.contains("Input is too long"));
        assert!(summary.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD"));
    }

    #[test]
    #[cfg(not(feature = "sensitive-logs"))]
    fn test_summarize_error_body_extracts_nested_message_and_reason() {
        let body = r#"{"error":{"message":"Improperly formed request","reason":"BAD_REQUEST"}}"#;
        let summary = KiroProvider::summarize_error_body(body);
        assert!(summary.contains("Improperly formed request"));
        assert!(summary.contains("BAD_REQUEST"));
    }

    #[test]
    #[cfg(not(feature = "sensitive-logs"))]
    fn test_summarize_error_body_truncates_long_text() {
        let body = "x".repeat(1000);
        let summary = KiroProvider::summarize_error_body(&body);
        assert!(summary.len() <= 256 + 3);
        assert!(summary.ends_with("..."));
    }

    #[test]
    fn test_ide_endpoint_inject_profile_arn_with_social_auth() {
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("social".to_string());
        credentials.profile_arn = Some("arn:aws:sso::111111111:profile/social-profile".to_string());

        let request_body =
            r#"{"conversationState":{},"profileArn":"arn:aws:sso::999999999:profile/old"}"#;
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["profileArn"].as_str().unwrap(),
            "arn:aws:sso::111111111:profile/social-profile"
        );
    }

    #[test]
    fn test_ide_endpoint_inject_profile_arn_idc_uses_existing_profile_arn() {
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("idc".to_string());
        credentials.profile_arn = Some("arn:aws:sso::111111111:profile/idc-profile".to_string());

        let request_body =
            r#"{"conversationState":{},"profileArn":"arn:aws:sso::999999999:profile/old"}"#;
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["profileArn"].as_str().unwrap(),
            "arn:aws:sso::111111111:profile/idc-profile"
        );
        assert!(parsed.get("conversationState").is_some());
    }

    #[test]
    fn test_ide_endpoint_inject_profile_arn_builder_id_uses_default_placeholder() {
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("builder-id".to_string());

        let request_body =
            r#"{"conversationState":{},"profileArn":"arn:aws:sso::999999999:profile/old"}"#;
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["profileArn"].as_str().unwrap(),
            SOCIAL_PROFILE_ARN
        );
    }

    #[test]
    fn test_ide_endpoint_inject_profile_arn_aws_sso_oidc_by_client_credentials() {
        let mut credentials = KiroCredentials::default();
        credentials.client_id = Some("client123".to_string());
        credentials.client_secret = Some("secret456".to_string());
        credentials.profile_arn = Some("arn:aws:sso::111111111:profile/test".to_string());

        let request_body =
            r#"{"conversationState":{},"profileArn":"arn:aws:sso::999999999:profile/old"}"#;
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["profileArn"].as_str().unwrap(),
            "arn:aws:sso::111111111:profile/test"
        );
    }

    #[test]
    fn test_ide_endpoint_inject_profile_arn_without_credential_arn() {
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("social".to_string());
        assert!(credentials.profile_arn.is_none());

        let request_body =
            r#"{"conversationState":{},"profileArn":"arn:aws:sso::999999999:profile/original"}"#;
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["profileArn"].as_str().unwrap(), SOCIAL_PROFILE_ARN);
    }

    #[test]
    fn test_ide_endpoint_inject_profile_arn_adds_missing_field() {
        let mut credentials = KiroCredentials::default();
        credentials.auth_method = Some("social".to_string());
        credentials.profile_arn = Some("arn:aws:sso::222222222:profile/new".to_string());

        let request_body = r#"{"conversationState":{"conversationId":"test"}}"#;
        let endpoint = IdeEndpoint::new();
        let machine_id = "a".repeat(64);
        let config = Config::default();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: &machine_id,
            config: &config,
        };
        let result = endpoint.transform_api_body(request_body, &ctx).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            parsed["profileArn"].as_str().unwrap(),
            "arn:aws:sso::222222222:profile/new"
        );
        assert_eq!(
            parsed["conversationState"]["conversationId"]
                .as_str()
                .unwrap(),
            "test"
        );
    }

    #[test]
    fn test_update_default_endpoint() {
        let mut config = Config::default();
        config.default_endpoint = "ide".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.endpoint = None; // 未显式指定，应使用默认值

        let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        endpoints.insert("ide".to_string(), Arc::new(IdeEndpoint::new()));
        endpoints.insert("ide-runtime".to_string(), Arc::new(IdeEndpoint::runtime()));
        endpoints.insert("cli".to_string(), Arc::new(CliEndpoint::new()));

        let tm =
            MultiTokenManager::new(config, vec![credentials.clone()], None, None, false).unwrap();
        let provider =
            KiroProvider::with_proxy(Arc::new(tm), None, endpoints.clone(), "ide".to_string());

        // 初始状态：默认 ide
        let endpoint = provider.endpoint_for(&credentials).unwrap();
        assert_eq!(endpoint.name(), "ide");

        // 热更新为 cli
        provider.update_default_endpoint("cli".to_string()).unwrap();
        let endpoint = provider.endpoint_for(&credentials).unwrap();
        assert_eq!(endpoint.name(), "cli");

        // 热更新为 ide-runtime
        provider
            .update_default_endpoint("ide-runtime".to_string())
            .unwrap();
        let endpoint = provider.endpoint_for(&credentials).unwrap();
        assert_eq!(endpoint.name(), "ide-runtime");

        // 热更新回 ide
        provider.update_default_endpoint("ide".to_string()).unwrap();
        let endpoint = provider.endpoint_for(&credentials).unwrap();
        assert_eq!(endpoint.name(), "ide");

        // 尝试更新为未知 endpoint，应返回错误
        let result = provider.update_default_endpoint("unknown".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("未知端点"));
    }

    #[test]
    fn test_endpoint_for_respects_credential_override() {
        let mut config = Config::default();
        config.default_endpoint = "ide".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.endpoint = Some("cli".to_string()); // 凭据显式指定 cli

        let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
        endpoints.insert("ide".to_string(), Arc::new(IdeEndpoint::new()));
        endpoints.insert("ide-runtime".to_string(), Arc::new(IdeEndpoint::runtime()));
        endpoints.insert("cli".to_string(), Arc::new(CliEndpoint::new()));

        let tm =
            MultiTokenManager::new(config, vec![credentials.clone()], None, None, false).unwrap();
        let provider = KiroProvider::with_proxy(Arc::new(tm), None, endpoints, "ide".to_string());

        // 凭据显式指定 cli，应优先使用凭据配置
        let endpoint = provider.endpoint_for(&credentials).unwrap();
        assert_eq!(endpoint.name(), "cli");

        // 即使热更新默认值为 ide，凭据显式配置仍生效
        provider.update_default_endpoint("ide".to_string()).unwrap();
        let endpoint = provider.endpoint_for(&credentials).unwrap();
        assert_eq!(endpoint.name(), "cli");

        // 即使热更新默认值为 runtime，凭据显式配置仍生效
        provider
            .update_default_endpoint("ide-runtime".to_string())
            .unwrap();
        let endpoint = provider.endpoint_for(&credentials).unwrap();
        assert_eq!(endpoint.name(), "cli");
    }
}
