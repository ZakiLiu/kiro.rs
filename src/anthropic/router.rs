//! Anthropic API 路由配置

use std::sync::Arc;

use axum::{
    Router,
    extract::DefaultBodyLimit,
    middleware,
    routing::{get, post},
};
use parking_lot::RwLock;

use crate::kiro::provider::KiroProvider;
use crate::metrics::MetricsCollector;
use crate::model::config::{CompressionConfig, Preset};

use super::{
    cross_request_cache::CrossRequestCache,
    handlers::{count_tokens, get_models, health, post_messages},
    middleware::{AppState, PromptCacheRuntime, auth_middleware, cors_layer},
};

/// 请求体最大大小限制 (50MB)
const MAX_BODY_SIZE: usize = 50 * 1024 * 1024;

/// 创建 Anthropic API 路由
///
/// # 端点
/// - `GET /v1/models` - 获取可用模型列表
/// - `POST /v1/messages` - 创建消息（对话）
/// - `POST /v1/messages/count_tokens` - 计算 token 数量
///
/// # 认证
/// 所有 `/v1` 路径需要 API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
///
/// # 参数
/// - `api_key`: API 密钥，用于验证客户端请求
/// - `kiro_provider`: 可选的 KiroProvider，用于调用上游 API
///
/// 创建带有 KiroProvider 的 Anthropic API 路由
#[allow(dead_code)]
#[allow(clippy::too_many_arguments)]
pub fn create_router_with_provider(
    api_key: impl Into<String>,
    kiro_provider: Option<Arc<KiroProvider>>,
    profile_arn: Option<String>,
    compression_config: Arc<RwLock<CompressionConfig>>,
    prompt_cache_runtime: Arc<RwLock<PromptCacheRuntime>>,
    metrics: Option<Arc<MetricsCollector>>,
    cross_request_cache: Option<Arc<CrossRequestCache>>,
    presets: Arc<RwLock<Vec<Preset>>>,
) -> Router {
    let mut state = AppState::new(api_key, prompt_cache_runtime);
    state = state.with_presets(presets);
    if let Some(provider) = kiro_provider {
        state = state.with_kiro_provider(provider);
    }
    if let Some(arn) = profile_arn {
        state = state.with_profile_arn(arn);
    }
    state = state.with_compression_config(compression_config);
    if let Some(m) = metrics {
        state = state.with_metrics(m);
    }
    if let Some(cache) = cross_request_cache {
        state = state.with_cross_request_cache(cache);
    }
    create_router_from_state(state)
}

/// 从已构造的 AppState 创建路由（供 main.rs 在注入运维组件后使用）
pub fn create_router_from_state(state: AppState) -> Router {
    // 需要认证的 /v1 路由（Claude + OpenAI 共享 /v1 前缀）
    let openai_routes = crate::openai::router::openai_routes();
    let v1_routes = Router::new()
        .route("/models", get(get_models))
        .route("/messages", post(post_messages))
        .route("/messages/count_tokens", post(count_tokens))
        .merge(openai_routes)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Gemini /v1beta 路由
    let gemini_routes = crate::gemini::router::gemini_routes().layer(
        middleware::from_fn_with_state(state.clone(), auth_middleware),
    );

    // Claude Code 别名：/anthropic/v1/messages → 与 /v1/messages 行为一致
    let anthropic_v1_routes = Router::new()
        .route("/messages", post(post_messages))
        .route("/messages/count_tokens", post(count_tokens))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        .nest("/v1", v1_routes)
        .nest("/anthropic/v1", anthropic_v1_routes)
        .nest("/v1beta", gemini_routes)
        .route("/health", get(health))
        .layer(cors_layer())
        .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
        .with_state(state)
}
