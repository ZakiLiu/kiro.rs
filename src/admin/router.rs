//! Admin API 路由配置

use axum::{
    Router, middleware,
    routing::{delete, get, post, put},
};

use super::{
    handlers::{
        add_credential, add_proxy, apply_image_update, assign_proxies_round_robin,
        assign_proxy_to_credential, batch_add_proxies, check_all_proxies, check_proxy,
        check_rate_limit, check_update, clear_throttle, complete_social_login,
        complete_social_relogin, create_client_key, create_group, create_preset, delete_client_key,
        delete_credential, delete_group, delete_preset, delete_proxy, disable_quota_exceeded,
        enable_overage_all, export_credentials, force_refresh_token, get_account_throttle_config,
        get_all_credentials, get_cached_balances, get_credential_balance, get_credential_models,
        get_global_config, get_global_proxy, get_load_balancing_mode, get_log_governance_config,
        get_metrics_by_credential, get_metrics_by_model, get_metrics_summary, get_presets,
        get_proxy_config, get_proxy_pool, get_update_config, import_token_json, list_client_keys,
        list_groups, list_traces, poll_external_idp_login, poll_idc_login, poll_idc_relogin,
        poll_social_login,
        poll_social_relogin, pull_update_image, reset_all_success_count, reset_client_key_stats,
        reset_failure_count, reset_success_count, rollback_image_update, rotate_client_key,
        set_account_throttle_config, set_client_key_disabled, set_credential_disabled,
        set_credential_endpoint, set_credential_overage, set_credential_priority,
        set_credential_region, set_global_proxy, set_load_balancing_mode,
        set_log_governance_config, set_proxy_enabled, set_update_config, social_oauth_callback,
        start_external_idp_login, start_idc_login, start_idc_relogin, start_social_login,
        start_social_relogin,
        stats_by_credential, stats_by_model, stats_overview, stats_timeseries, trace_failure_stats,
        update_admin_key, update_client_key, update_credential, update_global_config,
        update_group, update_group_config, get_group_config,
        update_preset, update_proxy_config, update_refresh_token,
    },
    middleware::{AdminState, admin_auth_middleware},
};

/// 创建 Admin API 路由
///
/// # 认证
/// 需要登录API密钥认证，支持：
/// - `x-api-key` header
/// - `Authorization: Bearer <token>` header
pub fn create_admin_router(state: AdminState) -> Router {
    // 需要登录API密钥认证的路由
    let authenticated = Router::new()
        // ---- 凭据 CRUD ----
        .route(
            "/credentials",
            get(get_all_credentials).post(add_credential),
        )
        .route("/credentials/balances/cached", get(get_cached_balances))
        .route("/credentials/export", get(export_credentials))
        .route("/credentials/import-token-json", post(import_token_json))
        .route("/credentials/reset-stats", post(reset_all_success_count))
        .route(
            "/credentials/disable-quota-exceeded",
            post(disable_quota_exceeded),
        )
        .route("/credentials/overage/enable-all", post(enable_overage_all))
        .route(
            "/credentials/{id}",
            delete(delete_credential).put(update_credential),
        )
        .route("/credentials/{id}/disabled", post(set_credential_disabled))
        .route("/credentials/{id}/priority", post(set_credential_priority))
        .route("/credentials/{id}/region", post(set_credential_region))
        .route("/credentials/{id}/endpoint", post(set_credential_endpoint))
        .route("/credentials/{id}/reset", post(reset_failure_count))
        .route("/credentials/{id}/clear-throttle", post(clear_throttle))
        .route("/credentials/{id}/reset-stats", post(reset_success_count))
        .route("/credentials/{id}/refresh", post(force_refresh_token))
        .route("/credentials/{id}/refresh-token", put(update_refresh_token))
        .route("/credentials/{id}/balance", get(get_credential_balance))
        .route("/credentials/{id}/overage", post(set_credential_overage))
        .route("/credentials/{id}/models", get(get_credential_models))
        .route("/credentials/{id}/proxy", post(assign_proxy_to_credential))
        // ---- 代理池 ----
        .route("/proxy-pool", get(get_proxy_pool).post(add_proxy))
        .route("/proxy-pool/batch", post(batch_add_proxies))
        .route("/proxy-pool/check-all", post(check_all_proxies))
        .route(
            "/proxy-pool/assign-round-robin",
            post(assign_proxies_round_robin),
        )
        .route("/proxy-pool/{id}", delete(delete_proxy))
        .route("/proxy-pool/{id}/enabled", post(set_proxy_enabled))
        .route("/proxy-pool/{id}/check", post(check_proxy))
        // ---- 旧代理配置（向后兼容） ----
        .route("/proxy", get(get_proxy_config).post(update_proxy_config))
        // ---- 配置管理 ----
        .route(
            "/config/load-balancing",
            get(get_load_balancing_mode).put(set_load_balancing_mode),
        )
        .route(
            "/config/account-throttle",
            get(get_account_throttle_config).put(set_account_throttle_config),
        )
        .route(
            "/config/log-governance",
            get(get_log_governance_config).put(set_log_governance_config),
        )
        .route(
            "/config/global-proxy",
            get(get_global_proxy).put(set_global_proxy),
        )
        .route(
            "/config/global",
            get(get_global_config).put(update_global_config),
        )
        .route("/config/admin-key", put(update_admin_key))
        // ---- 更新配置 ----
        .route(
            "/config/update",
            get(get_update_config).put(set_update_config),
        )
        // ---- 系统更新 ----
        .route("/system/update/check", get(check_update))
        .route("/system/update/pull", post(pull_update_image))
        .route("/system/update/apply", post(apply_image_update))
        .route("/system/update/rollback", post(rollback_image_update))
        .route("/system/update/rate-limit", post(check_rate_limit))
        // ---- Auth Flows ----
        .route("/auth/external-idp/start", post(start_external_idp_login))
        .route(
            "/auth/external-idp/poll/{session_id}",
            post(poll_external_idp_login),
        )
        .route("/auth/idc/start", post(start_idc_login))
        .route("/auth/idc/poll/{session_id}", post(poll_idc_login))
        .route("/auth/social/start", post(start_social_login))
        .route("/auth/social/poll/{session_id}", post(poll_social_login))
        .route(
            "/auth/social/complete/{session_id}",
            post(complete_social_login),
        )
        // ---- Relogin Flows ----
        .route(
            "/credentials/{id}/relogin/social/start",
            post(start_social_relogin),
        )
        .route(
            "/credentials/{id}/relogin/social/poll/{session_id}",
            post(poll_social_relogin),
        )
        .route(
            "/credentials/{id}/relogin/social/complete/{session_id}",
            post(complete_social_relogin),
        )
        .route(
            "/credentials/{id}/relogin/idc/start",
            post(start_idc_relogin),
        )
        .route(
            "/credentials/{id}/relogin/idc/poll/{session_id}",
            post(poll_idc_relogin),
        )
        // ---- 客户端 Key 管理 ----
        .route(
            "/client-keys",
            get(list_client_keys).post(create_client_key),
        )
        .route(
            "/client-keys/{id}",
            delete(delete_client_key).put(update_client_key),
        )
        .route("/client-keys/{id}/disabled", post(set_client_key_disabled))
        .route(
            "/client-keys/{id}/reset-stats",
            post(reset_client_key_stats),
        )
        .route("/client-keys/{id}/rotate", post(rotate_client_key))
        // ---- 账号分组 ----
        .route("/groups", get(list_groups).post(create_group))
        .route("/groups/{name}", delete(delete_group).patch(update_group))
        .route(
            "/groups/{name}/config",
            get(get_group_config).put(update_group_config),
        )
        // ---- 用量统计 ----
        .route("/stats/overview", get(stats_overview))
        .route("/stats/timeseries", get(stats_timeseries))
        .route("/stats/by-model", get(stats_by_model))
        .route("/stats/by-credential", get(stats_by_credential))
        // ---- 链路追踪 ----
        .route("/traces", get(list_traces))
        .route("/traces/failure-stats", get(trace_failure_stats))
        // ---- 旧指标（向后兼容） ----
        .route("/metrics/summary", get(get_metrics_summary))
        .route("/metrics/by-model", get(get_metrics_by_model))
        .route("/metrics/by-credential", get(get_metrics_by_credential))
        // ---- Prompt 预设 ----
        .route("/presets", get(get_presets).post(create_preset))
        .route("/presets/{id}", put(update_preset).delete(delete_preset))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            admin_auth_middleware,
        ));

    // 免鉴权路由：远程部署模式下 OAuth 公网回调（浏览器顶层导航到达，不带 admin API Key）。
    // 由 OAuth state 定位会话，CSRF 保护与本地回调服务器同等。
    let public = Router::new().route("/auth/callback/{*tail}", get(social_oauth_callback));

    Router::new()
        .merge(authenticated)
        .merge(public)
        .with_state(state)
}
