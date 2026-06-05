//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};

use super::{
    handlers::{
        add_credential, create_preset, delete_credential, delete_preset, force_refresh_token,
        get_all_credentials, get_cached_balances, get_credential_balance, get_global_config,
        get_metrics_by_credential, get_metrics_by_model, get_metrics_summary, get_presets,
        get_proxy_config, import_token_json, reset_failure_count, set_credential_disabled,
        set_credential_endpoint, set_credential_priority, set_credential_region, update_global_config,
        update_preset, update_proxy_config,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 端点
/// - `GET /credentials` - 获取所有凭据状态
/// - `POST /credentials` - 添加新凭据
/// - `POST /credentials/import-token-json` - 批量导入 token.json
/// - `DELETE /credentials/:id` - 删除凭据
/// - `POST /credentials/:id/disabled` - 设置凭据禁用状态
/// - `POST /credentials/:id/priority` - 设置凭据优先级
/// - `POST /credentials/:id/reset` - 重置失败计数
/// - `GET /credentials/:id/balance` - 获取凭据余额
/// - `GET /credentials/balances/cached` - 获取所有凭据的缓存余额
///
/// # 认证
/// 需要 Admin API Key 认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn create_admin_router(state: AdminState) -> Router {
    Router::new()
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/credentials/balances/cached", get(get_cached_balances))
        .route("/credentials/import-token-json", post(import_token_json))
        .route("/credentials/{id}", delete(delete_credential))
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/region", post(set_credential_region))
        .route("/credentials/{id}/endpoint", post(set_credential_endpoint))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/metrics/summary", get(get_metrics_summary))
        .route("/metrics/by-model", get(get_metrics_by_model))
        .route("/metrics/by-credential", get(get_metrics_by_credential))
        .route("/proxy", get(get_proxy_config).post(update_proxy_config))
        .route(
            "/config/global",
            get(get_global_config).put(update_global_config),
        )
        .route("/presets", get(get_presets).post(create_preset))
        .route("/presets/{id}", put(update_preset).delete(delete_preset))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ))
        .with_state(state)
}
