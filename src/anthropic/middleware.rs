//! Anthropic API 中间件

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use parking_lot::RwLock;

use crate::admin::client_keys::SharedClientKeyManager;
use crate::admin::trace_db::SharedTraceStore;
use crate::admin::usage_stats::{SharedAggregator, SharedRecorder};
use crate::common::auth;
use crate::kiro::provider::KiroProvider;
use crate::metrics::MetricsCollector;
use crate::model::config::{CompressionConfig, Preset, ToolCompatibilityMode};

use crate::admin::trace_db::TraceKeySource;

use super::cache_tracker::CacheTracker;
use super::cross_request_cache::CrossRequestCache;
use super::types::ErrorResponse;

/// 鉴权结果，注入到 request extension 中供 handler 读取
#[derive(Clone, Debug)]
pub struct AuthIdentity {
    pub key_id: u64,
    pub key_source: TraceKeySource,
    /// 客户端 Key 绑定的分组名（用于路由到对应分组的上游凭据）
    pub group: Option<String>,
}

#[derive(Clone)]
pub(crate) struct PromptCacheSnapshot {
    pub accounting_enabled: bool,
    pub ttl_seconds: u64,
    pub tracker: Arc<CacheTracker>,
}

pub struct PromptCacheRuntime {
    accounting_enabled: bool,
    ttl_seconds: u64,
    tracker: Arc<CacheTracker>,
    cache_dir: Option<PathBuf>,
}

impl PromptCacheRuntime {
    pub fn new(ttl_seconds: u64, accounting_enabled: bool, cache_dir: Option<PathBuf>) -> Self {
        let tracker = Arc::new(CacheTracker::new(
            Duration::from_secs(ttl_seconds),
            cache_dir.clone(),
        ));
        tracker.clone().spawn_background();
        Self {
            accounting_enabled,
            ttl_seconds,
            tracker,
            cache_dir,
        }
    }

    pub fn snapshot(&self) -> PromptCacheSnapshot {
        PromptCacheSnapshot {
            accounting_enabled: self.accounting_enabled,
            ttl_seconds: self.ttl_seconds,
            tracker: self.tracker.clone(),
        }
    }

    pub fn update(&mut self, ttl_seconds: Option<u64>, accounting_enabled: Option<bool>) {
        if let Some(value) = accounting_enabled {
            self.accounting_enabled = value;
        }

        if let Some(value) = ttl_seconds
            && self.ttl_seconds != value
        {
            self.tracker.flush_to_disk();
            self.ttl_seconds = value;
            let tracker = Arc::new(CacheTracker::new(
                Duration::from_secs(value),
                self.cache_dir.clone(),
            ));
            tracker.clone().spawn_background();
            self.tracker = tracker;
        }
    }
}

/// 应用共享状态
#[derive(Clone)]
pub struct AppState {
    /// API 密钥
    pub api_key: String,
    /// Kiro Provider（可选，用于实际 API 调用）
    /// 内部使用 MultiTokenManager，已支持线程安全的多凭据管理
    pub kiro_provider: Option<Arc<KiroProvider>>,
    /// Profile ARN（可选，用于请求）
    pub profile_arn: Option<String>,
    /// Claude Code 内置工具兼容模式。
    pub tool_compatibility_mode: ToolCompatibilityMode,
    /// 输入压缩配置（共享引用，支持热更新）
    pub compression_config: Arc<RwLock<CompressionConfig>>,
    /// Prompt Cache 运行时配置（共享引用，支持热更新）
    pub prompt_cache_runtime: Arc<RwLock<PromptCacheRuntime>>,
    /// 请求指标收集器（可选，禁用时为 None）
    pub metrics: Option<Arc<MetricsCollector>>,
    pub cross_request_cache: Option<Arc<CrossRequestCache>>,
    /// Prompt 预设列表（共享引用，支持 Admin API 运行时 CRUD）
    pub presets: Arc<RwLock<Vec<Preset>>>,
    /// 客户端 Key 管理器（鉴权 + 用量回写）
    pub client_keys: Option<SharedClientKeyManager>,
    /// 用量记录器（JSONL 落盘）
    pub usage_recorder: Option<SharedRecorder>,
    /// 用量聚合器（内存时序桶，供 Admin API 查询）
    pub usage_aggregator: Option<SharedAggregator>,
    /// 请求链路追踪（SQLite 落盘）
    pub trace_store: Option<SharedTraceStore>,
    /// Prompt 注入运行时配置（剥离限制 + preset 注入）
    pub prompt_config: crate::model::runtime::SharedPromptConfig,
}

impl AppState {
    /// 创建新的应用状态
    pub fn new(
        api_key: impl Into<String>,
        prompt_cache_runtime: Arc<RwLock<PromptCacheRuntime>>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            kiro_provider: None,
            profile_arn: None,
            tool_compatibility_mode: ToolCompatibilityMode::default(),
            compression_config: Arc::new(RwLock::new(CompressionConfig::default())),
            prompt_cache_runtime,
            metrics: None,
            cross_request_cache: None,
            presets: Arc::new(RwLock::new(Vec::new())),
            client_keys: None,
            usage_recorder: None,
            usage_aggregator: None,
            trace_store: None,
            prompt_config: Arc::new(parking_lot::RwLock::new(
                crate::model::runtime::PromptRuntimeConfig {
                    enabled: false,
                    enabled_presets: Vec::new(),
                    custom_content: None,
                    position: crate::model::config::SystemPromptPosition::Append,
                    strip_system_restrictions: false,
                },
            )),
        }
    }

    /// 设置 KiroProvider
    pub fn with_kiro_provider(mut self, provider: Arc<KiroProvider>) -> Self {
        self.kiro_provider = Some(provider);
        self
    }

    /// 设置 Profile ARN
    pub fn with_profile_arn(mut self, arn: impl Into<String>) -> Self {
        self.profile_arn = Some(arn.into());
        self
    }

    /// 设置 Claude Code 内置工具兼容模式。
    pub fn with_tool_compatibility_mode(mut self, mode: ToolCompatibilityMode) -> Self {
        self.tool_compatibility_mode = mode;
        self
    }

    /// 设置压缩配置（接受共享引用）
    pub fn with_compression_config(mut self, config: Arc<RwLock<CompressionConfig>>) -> Self {
        self.compression_config = config;
        self
    }

    /// 设置指标收集器
    pub fn with_metrics(mut self, metrics: Arc<MetricsCollector>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    pub fn with_cross_request_cache(mut self, cache: Arc<CrossRequestCache>) -> Self {
        self.cross_request_cache = Some(cache);
        self
    }

    /// 设置 Prompt 预设（接受共享引用，支持 Admin API 运行时 CRUD）
    pub fn with_presets(mut self, presets: Arc<RwLock<Vec<Preset>>>) -> Self {
        self.presets = presets;
        self
    }

    pub fn with_client_keys(mut self, mgr: SharedClientKeyManager) -> Self {
        self.client_keys = Some(mgr);
        self
    }

    pub fn with_usage_recorder(mut self, rec: SharedRecorder) -> Self {
        self.usage_recorder = Some(rec);
        self
    }

    pub fn with_usage_aggregator(mut self, agg: SharedAggregator) -> Self {
        self.usage_aggregator = Some(agg);
        self
    }

    pub fn with_trace_store(mut self, store: SharedTraceStore) -> Self {
        self.trace_store = Some(store);
        self
    }

    pub fn with_prompt_config(mut self, cfg: crate::model::runtime::SharedPromptConfig) -> Self {
        self.prompt_config = cfg;
        self
    }

    pub fn prompt_cache_snapshot(&self) -> PromptCacheSnapshot {
        self.prompt_cache_runtime.read().snapshot()
    }
}

/// API Key 认证中间件（双轨：master apiKey + 客户端 Key）
pub async fn auth_middleware(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let key = match auth::extract_api_key(&request) {
        Some(k) if !k.is_empty() => k,
        _ => {
            let error = ErrorResponse::authentication_error();
            return (StatusCode::UNAUTHORIZED, Json(error)).into_response();
        }
    };

    // 所有 Key 统一走客户端 Key 管理器校验（master apiKey 已通过 ensure_system_key 注册为系统 Key）
    if let Some(ref mgr) = state.client_keys
        && let Some(id) = mgr.verify_and_touch(&key)
    {
        let group = mgr.group_of(id);
        request.extensions_mut().insert(AuthIdentity {
            key_id: id,
            key_source: TraceKeySource::ClientKey,
            group,
        });
        return next.run(request).await;
    }

    // 兜底：ClientKeyManager 未初始化时回退 master apiKey 直接比对
    if auth::constant_time_eq(&key, &state.api_key) {
        request.extensions_mut().insert(AuthIdentity {
            key_id: 0,
            key_source: TraceKeySource::MasterApiKey,
            group: None,
        });
        return next.run(request).await;
    }

    let error = ErrorResponse::authentication_error();
    (StatusCode::UNAUTHORIZED, Json(error)).into_response()
}

/// CORS 中间件层
///
/// **安全说明 (SEC-003, 有意设计)**：当前配置允许所有来源（Any），这是为了支持公开 API 服务。
/// 此处宽松 CORS **不构成 CSRF 风险**：本服务认证完全基于显式请求头
/// （`x-api-key` / `Authorization: Bearer`），不依赖 Cookie 等浏览器自动携带的环境凭据，
/// 因此跨站请求无法借用受害者的认证态。若未来引入基于 Cookie 的会话，必须收紧 CORS 并加 CSRF 防护。
/// 如需更严格的安全控制，请根据实际需求配置具体的允许来源、方法和头信息。
///
/// # 配置说明
/// - `allow_origin(Any)`: 允许任何来源的请求
/// - `allow_methods(Any)`: 允许任何 HTTP 方法
/// - `allow_headers(Any)`: 允许任何请求头
pub fn cors_layer() -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};

    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
