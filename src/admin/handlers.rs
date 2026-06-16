//! Admin API HTTP 处理器

use std::collections::HashMap;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};

use super::{
    client_keys::mask_client_key,
    middleware::AdminState,
    trace_db::TraceQuery,
    types::{
        AddCredentialRequest, AddProxyRequest, AssignProxyRequest, AssignRoundRobinRequest,
        BatchAddProxyRequest, ClientKeyItem, ClientKeysResponse, CompleteSocialLoginRequest,
        CreateClientKeyRequest, CreateClientKeyResponse, CreatePresetRequest,
        GlobalProxyResponse, ImportTokenJsonRequest, SetAccountThrottleConfigRequest,
        SetDisabledRequest, SetEndpointRequest, SetLoadBalancingModeRequest,
        SetLogGovernanceConfigRequest, SetPriorityRequest, SetRegionRequest,
        StartIdcLoginRequest, StartSocialLoginRequest, SuccessResponse,
        UpdateAdminKeyRequest, UpdateClientKeyRequest, UpdateCredentialRequest,
        UpdatePresetRequest, UpdateProxyConfigRequest, UpdateRefreshTokenRequest,
    },
    usage_stats::{Range, StatsGranularity, StatsQueryWindow},
};

// Path 元组提取：(credential_id, session_id)
type CredSessionPath = (u64, String);

/// GET /api/admin/credentials
/// 获取所有凭据状态
pub async fn get_all_credentials(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_all_credentials();
    Json(response)
}

/// POST /api/admin/credentials/:id/disabled
/// 设置凭据禁用状态
pub async fn set_credential_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    match state.service.set_disabled(id, payload.disabled) {
        Ok(_) => {
            let action = if payload.disabled { "禁用" } else { "启用" };
            Json(SuccessResponse::new(format!("凭据 #{} 已{}", id, action))).into_response()
        }
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/priority
/// 设置凭据优先级
pub async fn set_credential_priority(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetPriorityRequest>,
) -> impl IntoResponse {
    match state.service.set_priority(id, payload.priority) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 优先级已设置为 {}",
            id, payload.priority
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/region
/// 设置凭据 Region
pub async fn set_credential_region(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetRegionRequest>,
) -> impl IntoResponse {
    match state
        .service
        .set_region(id, payload.region, payload.api_region)
    {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} Region 已更新", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/endpoint
/// 设置凭据 endpoint
pub async fn set_credential_endpoint(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetEndpointRequest>,
) -> impl IntoResponse {
    match state.service.set_endpoint(id, payload.endpoint) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} endpoint 已更新",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/export
pub async fn export_credentials(
    State(state): State<AdminState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let id_filter: Option<std::collections::HashSet<u64>> = params
        .get("ids")
        .map(|raw| {
            raw.split(',')
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .collect::<std::collections::HashSet<u64>>()
        })
        .filter(|s| !s.is_empty());
    let response = state.service.export_credentials(id_filter.as_ref());
    Json(response)
}

/// POST /api/admin/credentials/disable-quota-exceeded
pub async fn disable_quota_exceeded(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.disable_quota_exceeded()).into_response()
}

/// POST /api/admin/credentials/:id/overage
pub async fn set_credential_overage(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<super::types::SetOverageRequest>,
) -> impl IntoResponse {
    match state.service.set_overage(id, payload.enabled).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 已{}超额",
            id,
            if payload.enabled { "开启" } else { "关闭" }
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/overage/enable-all
pub async fn enable_overage_all(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.enable_overage_for_all_capable().await).into_response()
}

/// POST /api/admin/credentials/:id/reset
/// 重置失败计数并重新启用
pub async fn reset_failure_count(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_and_enable(id) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} 失败计数已重置并重新启用",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/clear-throttle
/// 手动解除凭据的账号级风控冷却
pub async fn clear_throttle(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.clear_throttle(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 风控冷却已解除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/refresh
/// 强制刷新指定凭据 Token
pub async fn force_refresh_token(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.force_refresh_token(id).await {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} Token 已强制刷新",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/models
pub async fn get_credential_models(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_available_models(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/:id/balance
/// 获取指定凭据的余额
pub async fn get_credential_balance(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.get_balance(id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/credentials/balances/cached
/// 获取所有凭据的缓存余额
pub async fn get_cached_balances(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_cached_balances())
}

/// POST /api/admin/credentials
/// 添加新凭据
pub async fn add_credential(
    State(state): State<AdminState>,
    Json(payload): Json<AddCredentialRequest>,
) -> impl IntoResponse {
    match state.service.add_credential(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// DELETE /api/admin/credentials/:id
/// 删除凭据
pub async fn delete_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_credential(id) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/credentials/:id
/// 更新凭据可编辑字段（email、proxy 等）
pub async fn update_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateCredentialRequest>,
) -> impl IntoResponse {
    match state.service.update_credential(id, payload) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 已更新", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/credentials/:id/refresh-token
/// 更新已禁用凭据的 refreshToken
pub async fn update_refresh_token(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateRefreshTokenRequest>,
) -> impl IntoResponse {
    match state.service.update_refresh_token(id, payload) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} refreshToken 已更新（当前仍为禁用状态，请手动启用）",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/import-token-json
/// 批量导入 token.json
pub async fn import_token_json(
    State(state): State<AdminState>,
    Json(payload): Json<ImportTokenJsonRequest>,
) -> impl IntoResponse {
    let response = state.service.import_token_json(payload).await;
    Json(response)
}

/// POST /api/admin/credentials/reset-stats
/// 重置所有凭据的 success_count
pub async fn reset_all_success_count(State(state): State<AdminState>) -> impl IntoResponse {
    match state.service.reset_success_count(None) {
        Ok(count) => Json(SuccessResponse::new(format!(
            "已重置 {} 个凭据的 success_count",
            count
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/reset-stats
/// 重置指定凭据的 success_count
pub async fn reset_success_count(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.reset_success_count(Some(id)) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "凭据 #{} success_count 已重置",
            id
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /proxy - 获取全局代理配置
pub async fn get_proxy_config(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_proxy_config())
}

/// POST /proxy - 更新全局代理配置
pub async fn update_proxy_config(
    State(state): State<AdminState>,
    Json(req): Json<UpdateProxyConfigRequest>,
) -> impl IntoResponse {
    match state.service.update_proxy_config(req).await {
        Ok(_) => Json(SuccessResponse::new("全局代理配置已更新")).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/metrics/summary - 获取指标概览
pub async fn get_metrics_summary(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.metrics_summary())
}

/// GET /api/admin/metrics/by-model - 获取按模型聚合的指标
pub async fn get_metrics_by_model(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.metrics_by_model())
}

/// GET /api/admin/metrics/by-credential - 获取按凭据聚合的指标
pub async fn get_metrics_by_credential(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.metrics_by_credential())
}

/// GET /api/admin/config/global - 获取全局配置
pub async fn get_global_config(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_global_config();
    Json(response)
}

/// PUT /api/admin/config/global - 更新全局配置
pub async fn update_global_config(
    State(state): State<AdminState>,
    Json(req): Json<super::types::UpdateGlobalConfigRequest>,
) -> impl IntoResponse {
    match state.service.update_global_config(req).await {
        Ok(_) => Json(SuccessResponse::new("全局配置已更新")).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

// ============ Prompt 预设 CRUD ============

/// GET /api/admin/presets - 获取所有预设
pub async fn get_presets(State(state): State<AdminState>) -> impl IntoResponse {
    let presets = state.presets.read().clone();
    Json(presets)
}

/// POST /api/admin/presets - 创建新预设
pub async fn create_preset(
    State(state): State<AdminState>,
    Json(payload): Json<CreatePresetRequest>,
) -> impl IntoResponse {
    let mut presets = state.presets.write();

    // 检查 ID 唯一性
    if presets.iter().any(|p| p.id == payload.id) {
        return (
            StatusCode::CONFLICT,
            Json(super::types::AdminErrorResponse::invalid_request(format!(
                "预设 ID '{}' 已存在",
                payload.id
            ))),
        )
            .into_response();
    }

    let preset = crate::model::config::Preset {
        id: payload.id,
        name: payload.name,
        system_prompt: payload.system_prompt,
        enabled: payload.enabled,
    };

    presets.push(preset.clone());
    tracing::info!(preset_id = %preset.id, "已创建 Prompt Preset");

    (StatusCode::CREATED, Json(preset)).into_response()
}

/// PUT /api/admin/presets/:id - 更新预设
pub async fn update_preset(
    State(state): State<AdminState>,
    Path(id): Path<String>,
    Json(payload): Json<UpdatePresetRequest>,
) -> impl IntoResponse {
    let mut presets = state.presets.write();

    let Some(preset) = presets.iter_mut().find(|p| p.id == id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "预设 '{}' 不存在",
                id
            ))),
        )
            .into_response();
    };

    if let Some(name) = payload.name {
        preset.name = name;
    }
    if let Some(system_prompt) = payload.system_prompt {
        preset.system_prompt = system_prompt;
    }
    if let Some(enabled) = payload.enabled {
        preset.enabled = enabled;
    }

    let updated = preset.clone();
    tracing::info!(preset_id = %id, "已更新 Prompt Preset");

    Json(updated).into_response()
}

/// DELETE /api/admin/presets/:id - 删除预设
pub async fn delete_preset(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let mut presets = state.presets.write();
    let before = presets.len();
    presets.retain(|p| p.id != id);

    if presets.len() == before {
        return (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "预设 '{}' 不存在",
                id
            ))),
        )
            .into_response();
    }

    tracing::info!(preset_id = %id, "已删除 Prompt Preset");
    StatusCode::NO_CONTENT.into_response()
}

// ============ 代理池管理 ============

/// GET /api/admin/proxy-pool
/// 获取代理池列表
pub async fn get_proxy_pool(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_proxy_pool();
    Json(response)
}

/// POST /api/admin/proxy-pool
/// 添加代理到池中
pub async fn add_proxy(
    State(state): State<AdminState>,
    Json(payload): Json<AddProxyRequest>,
) -> impl IntoResponse {
    match state.service.add_proxy(payload.url, payload.label) {
        Ok(entry) => Json(entry).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxy-pool/batch
/// 批量添加代理
pub async fn batch_add_proxies(
    State(state): State<AdminState>,
    Json(payload): Json<BatchAddProxyRequest>,
) -> impl IntoResponse {
    let (added, errors) = state.service.batch_add_proxies(payload);
    Json(serde_json::json!({
        "added": added.len(),
        "errors": errors.len(),
        "proxies": added,
        "errorMessages": errors
    }))
}

/// DELETE /api/admin/proxy-pool/:id
/// 删除代理
pub async fn delete_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.delete_proxy(id) {
        Ok(_) => Json(SuccessResponse::new(format!("代理 #{} 已删除", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxy-pool/:id/enabled
/// 设置代理启用/禁用
pub async fn set_proxy_enabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let enabled = payload
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    match state.service.set_proxy_enabled(id, enabled) {
        Ok(_) => Json(SuccessResponse::new(format!(
            "代理 #{} 已{}",
            id,
            if enabled { "启用" } else { "禁用" }
        )))
        .into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/proxy
/// 将代理池中的代理分配给凭据
pub async fn assign_proxy_to_credential(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<AssignProxyRequest>,
) -> impl IntoResponse {
    match state.service.assign_proxy_to_credential(id, payload) {
        Ok(_) => Json(SuccessResponse::new(format!("凭据 #{} 代理已更新", id))).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxy-pool/:id/check
/// 即时探测单个代理的连通性
pub async fn check_proxy(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    match state.service.check_proxy(id).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/proxy-pool/check-all
/// 触发全部代理的健康检查
pub async fn check_all_proxies(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.check_all_proxies().await)
}

/// POST /api/admin/proxy-pool/assign-round-robin
/// 将可用代理轮询批量分配给凭据
pub async fn assign_proxies_round_robin(
    State(state): State<AdminState>,
    Json(payload): Json<AssignRoundRobinRequest>,
) -> impl IntoResponse {
    match state
        .service
        .assign_proxies_round_robin(payload.credential_ids)
    {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

// ============ 配置管理 ============

/// GET /api/admin/config/load-balancing
/// 获取负载均衡模式
pub async fn get_load_balancing_mode(State(state): State<AdminState>) -> impl IntoResponse {
    let response = state.service.get_load_balancing_mode();
    Json(response)
}

/// PUT /api/admin/config/load-balancing
/// 设置负载均衡模式
pub async fn set_load_balancing_mode(
    State(state): State<AdminState>,
    Json(payload): Json<SetLoadBalancingModeRequest>,
) -> impl IntoResponse {
    match state.service.set_load_balancing_mode(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/account-throttle
/// 获取账号级风控故障转移配置
pub async fn get_account_throttle_config(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_account_throttle_config())
}

/// PUT /api/admin/config/account-throttle
/// 更新账号级风控故障转移配置
pub async fn set_account_throttle_config(
    State(state): State<AdminState>,
    Json(payload): Json<SetAccountThrottleConfigRequest>,
) -> impl IntoResponse {
    match state.service.set_account_throttle_config(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/log-governance
/// 获取日志治理配置（trace 开关 / trace 保留 / usage 保留）
pub async fn get_log_governance_config(State(state): State<AdminState>) -> impl IntoResponse {
    Json(state.service.get_log_governance_config())
}

/// PUT /api/admin/config/log-governance
/// 更新日志治理配置（运行时生效 + 持久化 config.json）
pub async fn set_log_governance_config(
    State(state): State<AdminState>,
    Json(payload): Json<SetLogGovernanceConfigRequest>,
) -> impl IntoResponse {
    match state.service.set_log_governance_config(payload) {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// GET /api/admin/config/global-proxy
/// 获取当前全局代理配置
pub async fn get_global_proxy(State(state): State<AdminState>) -> impl IntoResponse {
    Json(GlobalProxyResponse {
        proxy_url: state.service.get_global_proxy(),
    })
}

/// PUT /api/admin/config/global-proxy
/// 设置或清除全局代理配置
pub async fn set_global_proxy(
    State(state): State<AdminState>,
    Json(payload): Json<super::types::SetGlobalProxyRequest>,
) -> impl IntoResponse {
    match state.service.set_global_proxy(payload.proxy_url) {
        Ok(_) => Json(SuccessResponse::new("全局代理已更新")).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// PUT /api/admin/config/admin-key
/// 修改登录API密钥（adminApiKey）并持久化到配置文件。
pub async fn update_admin_key(
    State(state): State<AdminState>,
    Json(payload): Json<UpdateAdminKeyRequest>,
) -> impl IntoResponse {
    let new_key = payload.new_key.trim().to_string();
    if new_key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(super::types::AdminErrorResponse::invalid_request(
                "新登录API密钥不能为空",
            )),
        )
            .into_response();
    }

    // 更新内存中的登录API密钥
    *state.admin_api_key.write() = new_key.clone();

    // 通过 service 持久化到 config.json
    state.service.persist_admin_key(&new_key);

    Json(SuccessResponse::new("登录API密钥已更新")).into_response()
}

// ============ Auth Flow STUBS ============
// Auth flow handlers (social login, IdC login, relogin) depend on auth/social.rs and auth/idc.rs
// which are not yet ported. These stubs return 501 Not Implemented for now.

/// POST /api/admin/auth/idc/start
/// 发起 IdC 设备授权登录（STUB）
pub async fn start_idc_login(
    State(state): State<AdminState>,
    Json(payload): Json<StartIdcLoginRequest>,
) -> impl IntoResponse {
    match state.service.start_idc_login(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/auth/idc/poll/:session_id
/// 轮询 IdC 登录状态（STUB）
pub async fn poll_idc_login(
    State(state): State<AdminState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match state.service.poll_idc_login(&session_id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/auth/social/start
/// 发起 Social 登录，返回 portal URL（STUB）
pub async fn start_social_login(
    State(state): State<AdminState>,
    Json(payload): Json<StartSocialLoginRequest>,
) -> impl IntoResponse {
    match state.service.start_social_login(payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/auth/social/poll/:session_id
/// 轮询 Social 登录状态（STUB）
pub async fn poll_social_login(
    State(state): State<AdminState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    match state.service.poll_social_login(&session_id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/auth/social/complete/:session_id
/// 远程模式下手动完成 Social 登录（STUB）
pub async fn complete_social_login(
    State(state): State<AdminState>,
    Path(session_id): Path<String>,
    Json(payload): Json<CompleteSocialLoginRequest>,
) -> impl IntoResponse {
    match state
        .service
        .complete_social_login(
            &session_id,
            payload.code,
            payload.state,
            payload.login_option,
            payload.path,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/relogin/social/start
/// 发起 Social 重新登录（STUB）
pub async fn start_social_relogin(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<StartSocialLoginRequest>,
) -> impl IntoResponse {
    match state.service.start_social_relogin(id, payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/relogin/social/poll/:session_id
/// 轮询 Social 重新登录状态（STUB）
pub async fn poll_social_relogin(
    State(state): State<AdminState>,
    Path((_, session_id)): Path<CredSessionPath>,
) -> impl IntoResponse {
    match state.service.poll_social_login(&session_id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/relogin/social/complete/:session_id
/// 远程模式下手动完成 Social 重新登录（STUB）
pub async fn complete_social_relogin(
    State(state): State<AdminState>,
    Path((_, session_id)): Path<CredSessionPath>,
    Json(payload): Json<CompleteSocialLoginRequest>,
) -> impl IntoResponse {
    match state
        .service
        .complete_social_login(
            &session_id,
            payload.code,
            payload.state,
            payload.login_option,
            payload.path,
        )
        .await
    {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/relogin/idc/start
/// 发起 IdC 重新登录（STUB）
pub async fn start_idc_relogin(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<StartIdcLoginRequest>,
) -> impl IntoResponse {
    match state.service.start_idc_relogin(id, payload).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

/// POST /api/admin/credentials/:id/relogin/idc/poll/:session_id
/// 轮询 IdC 重新登录状态（STUB）
pub async fn poll_idc_relogin(
    State(state): State<AdminState>,
    Path((_, session_id)): Path<CredSessionPath>,
) -> impl IntoResponse {
    match state.service.poll_idc_login(&session_id).await {
        Ok(response) => Json(response).into_response(),
        Err(e) => (e.status_code(), Json(e.into_response())).into_response(),
    }
}

// ============ 客户端 API Key 分发 ============

fn key_to_item(k: &super::client_keys::ClientKey) -> ClientKeyItem {
    ClientKeyItem {
        id: k.id,
        masked_key: mask_client_key(&k.key),
        name: k.name.clone(),
        description: k.description.clone(),
        disabled: k.disabled,
        created_at: k.created_at.clone(),
        last_used_at: k.last_used_at.clone(),
        total_calls: k.total_calls,
        total_input_tokens: k.total_input_tokens,
        total_output_tokens: k.total_output_tokens,
        total_cache_creation_tokens: k.total_cache_creation_tokens,
        total_cache_read_tokens: k.total_cache_read_tokens,
        group: k.group.clone(),
        is_system: k.is_system,
    }
}

/// 获取 client_keys 管理器，若未初始化返回 501
fn require_client_keys(state: &AdminState) -> Result<&super::client_keys::SharedClientKeyManager, axum::response::Response> {
    state.client_keys.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(super::types::AdminErrorResponse::internal_error("Client Key 管理器未初始化")),
        )
            .into_response()
    })
}

/// GET /api/admin/client-keys
pub async fn list_client_keys(State(state): State<AdminState>) -> impl IntoResponse {
    let client_keys = match require_client_keys(&state) {
        Ok(ck) => ck,
        Err(resp) => return resp,
    };
    let keys = client_keys.list();
    let items: Vec<ClientKeyItem> = keys.iter().map(key_to_item).collect();
    Json(ClientKeysResponse {
        total: items.len(),
        keys: items,
    })
    .into_response()
}

/// POST /api/admin/client-keys
pub async fn create_client_key(
    State(state): State<AdminState>,
    Json(payload): Json<CreateClientKeyRequest>,
) -> impl IntoResponse {
    let client_keys = match require_client_keys(&state) {
        Ok(ck) => ck,
        Err(resp) => return resp,
    };
    let name = payload.name.trim();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(super::types::AdminErrorResponse::invalid_request(
                "name 不能为空",
            )),
        )
            .into_response();
    }
    let entry = client_keys.create(
        name.to_string(),
        payload
            .description
            .map(|d| d.trim().to_string())
            .filter(|d| !d.is_empty()),
        payload
            .group
            .map(|g| g.trim().to_string())
            .filter(|g| !g.is_empty()),
    );
    Json(CreateClientKeyResponse {
        id: entry.id,
        key: entry.key,
        name: entry.name,
        created_at: entry.created_at,
    })
    .into_response()
}

/// DELETE /api/admin/client-keys/:id
pub async fn delete_client_key(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let client_keys = match require_client_keys(&state) {
        Ok(ck) => ck,
        Err(resp) => return resp,
    };
    if client_keys.is_system(id) {
        return (
            StatusCode::CONFLICT,
            Json(super::types::AdminErrorResponse::invalid_request(
                "系统密钥（config.json apiKey）不可删除",
            )),
        )
            .into_response();
    }
    if client_keys.delete(id) {
        Json(SuccessResponse::new(format!("Key #{} 已删除", id))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "Key #{} 不存在",
                id
            ))),
        )
            .into_response()
    }
}

/// PUT /api/admin/client-keys/:id
pub async fn update_client_key(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<UpdateClientKeyRequest>,
) -> impl IntoResponse {
    let client_keys = match require_client_keys(&state) {
        Ok(ck) => ck,
        Err(resp) => return resp,
    };
    let description = payload
        .description
        .map(|d| if d.is_empty() { None } else { Some(d) });
    let group = payload
        .group
        .map(|g| {
            let t = g.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        });
    if client_keys.update_meta(id, payload.name, description, group) {
        Json(SuccessResponse::new(format!("Key #{} 已更新", id))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "Key #{} 不存在",
                id
            ))),
        )
            .into_response()
    }
}

/// POST /api/admin/client-keys/:id/disabled
pub async fn set_client_key_disabled(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
    Json(payload): Json<SetDisabledRequest>,
) -> impl IntoResponse {
    let client_keys = match require_client_keys(&state) {
        Ok(ck) => ck,
        Err(resp) => return resp,
    };
    if client_keys.set_disabled(id, payload.disabled) {
        let action = if payload.disabled { "禁用" } else { "启用" };
        Json(SuccessResponse::new(format!("Key #{} 已{}", id, action))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "Key #{} 不存在",
                id
            ))),
        )
            .into_response()
    }
}

/// POST /api/admin/client-keys/:id/reset-stats
pub async fn reset_client_key_stats(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let client_keys = match require_client_keys(&state) {
        Ok(ck) => ck,
        Err(resp) => return resp,
    };
    if client_keys.reset_stats(id) {
        Json(SuccessResponse::new(format!("Key #{} 统计已重置", id))).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "Key #{} 不存在",
                id
            ))),
        )
            .into_response()
    }
}

/// POST /api/admin/client-keys/:id/rotate
///
/// 轮换 Key 值：旧明文立即失效，生成新明文返回（仅此一次可见）。
pub async fn rotate_client_key(
    State(state): State<AdminState>,
    Path(id): Path<u64>,
) -> impl IntoResponse {
    let client_keys = match require_client_keys(&state) {
        Ok(ck) => ck,
        Err(resp) => return resp,
    };
    match client_keys.rotate(id) {
        Some(entry) => {
            // 系统密钥轮换后明文变了，需同步写回 config.json apiKey
            if entry.is_system {
                state.service.persist_api_key(&entry.key);
            }
            Json(CreateClientKeyResponse {
                id: entry.id,
                key: entry.key,
                name: entry.name,
                created_at: entry.created_at,
            })
            .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "Key #{} 不存在",
                id
            ))),
        )
            .into_response(),
    }
}

// ============ 用量统计 ============

fn parse_range(params: &std::collections::HashMap<String, String>) -> Result<Range, String> {
    let Some(range) = params.get("range") else {
        return Err("range 必须是 24h、7d 或 30d".to_string());
    };
    Range::parse(range.as_str()).ok_or_else(|| "range 必须是 24h、7d 或 30d".to_string())
}

fn parse_key_id(params: &HashMap<String, String>) -> Result<Option<u64>, String> {
    match params.get("keyId") {
        Some(s) => s
            .parse::<u64>()
            .map(Some)
            .map_err(|_| "keyId 必须是数字".to_string()),
        None => Ok(None),
    }
}

/// 解析可选的分组筛选参数。空字符串视为不传。
fn parse_group_filter(params: &HashMap<String, String>) -> Option<String> {
    params
        .get("group")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 把 group 名转换为该分组下所有凭据 id 的白名单
fn group_to_cred_ids(
    _state: &AdminState,
    group: Option<&str>,
) -> Option<std::collections::HashSet<u64>> {
    let _g = group?;
    // TODO: CredentialStatusItem 尚无 groups 字段，group 过滤暂返回空集（fail-closed）
    Some(std::collections::HashSet::new())
}

fn parse_granularity(params: &HashMap<String, String>) -> Result<StatsGranularity, String> {
    match params.get("granularity") {
        Some(s) => {
            StatsGranularity::parse(s).ok_or_else(|| "granularity 必须是 hour 或 day".to_string())
        }
        None => Err("granularity 必须是 hour 或 day".to_string()),
    }
}

fn parse_stats_window(params: &HashMap<String, String>) -> Result<StatsQueryWindow, String> {
    let granularity = parse_granularity(params)?;
    match (params.get("startDate"), params.get("endDate")) {
        (Some(start), Some(end)) => custom_stats_window(start, end, granularity),
        (None, None) => Ok(StatsQueryWindow::preset(parse_range(params)?, granularity)),
        _ => Err("startDate 和 endDate 必须同时提供".to_string()),
    }
}

fn custom_stats_window(
    start: &str,
    end: &str,
    granularity: StatsGranularity,
) -> Result<StatsQueryWindow, String> {
    let start_date = parse_stats_date(start, "startDate")?;
    let end_date = parse_stats_date(end, "endDate")?;
    if end_date < start_date {
        return Err("endDate 不能早于 startDate".to_string());
    }
    let start_ts = local_midnight_ts(start_date)?;
    let end_ts = local_midnight_ts(end_date + Duration::days(1))?;
    Ok(StatsQueryWindow {
        start_ts,
        end_ts,
        granularity,
    })
}

fn parse_stats_date(value: &str, name: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|_| format!("{} 必须使用 YYYY-MM-DD 格式", name))
}

fn local_midnight_ts(date: NaiveDate) -> Result<i64, String> {
    Local
        .with_ymd_and_hms(date.year(), date.month(), date.day(), 0, 0, 0)
        .single()
        .map(|d| d.timestamp())
        .ok_or_else(|| format!("日期 {} 无法转换为本地时间", date))
}

fn stats_query_parts(
    params: &HashMap<String, String>,
) -> Result<(StatsQueryWindow, Option<u64>), String> {
    Ok((parse_stats_window(params)?, parse_key_id(params)?))
}

fn stats_bad_request(message: String) -> axum::response::Response {
    (
        StatusCode::BAD_REQUEST,
        Json(super::types::AdminErrorResponse::invalid_request(message)),
    )
        .into_response()
}

/// 获取 usage_aggregator，若未初始化返回 501
fn require_usage_aggregator(state: &AdminState) -> Result<&super::usage_stats::SharedAggregator, axum::response::Response> {
    state.usage_aggregator.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(super::types::AdminErrorResponse::internal_error("用量聚合器未初始化")),
        )
            .into_response()
    })
}

/// 获取 trace_store，若未初始化返回 501
fn require_trace_store(state: &AdminState) -> Result<&super::trace_db::SharedTraceStore, axum::response::Response> {
    state.trace_store.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(super::types::AdminErrorResponse::internal_error("链路追踪存储未初始化")),
        )
            .into_response()
    })
}

/// 获取 groups 管理器，若未初始化返回 501
fn require_groups(state: &AdminState) -> Result<&super::groups::SharedGroupManager, axum::response::Response> {
    state.groups.as_ref().ok_or_else(|| {
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(super::types::AdminErrorResponse::internal_error("分组管理器未初始化")),
        )
            .into_response()
    })
}

/// GET /api/admin/stats/overview
pub async fn stats_overview(State(state): State<AdminState>) -> impl IntoResponse {
    let usage_aggregator = match require_usage_aggregator(&state) {
        Ok(ua) => ua,
        Err(resp) => return resp,
    };
    let overview = usage_aggregator.overview();
    let active_keys = state.client_keys.as_ref().map(|ck| ck.active_count() as u64).unwrap_or(0);
    let snapshot = state.service.get_all_credentials();
    let active_credentials = snapshot.credentials.iter().filter(|c| !c.disabled).count() as u64;
    let response = serde_json::json!({
        "todayCalls": overview.today_calls,
        "todayInputTokens": overview.today_input_tokens,
        "todayOutputTokens": overview.today_output_tokens,
        "todayErrors": overview.today_errors,
        "todayCredits": overview.today_credits,
        "weekCalls": overview.week_calls,
        "weekInputTokens": overview.week_input_tokens,
        "weekOutputTokens": overview.week_output_tokens,
        "weekCredits": overview.week_credits,
        "activeClientKeys": active_keys,
        "activeCredentials": active_credentials,
    });
    Json(response).into_response()
}

/// GET /api/admin/stats/timeseries?range=24h|7d|30d&granularity=hour|day&group=...
pub async fn stats_timeseries(
    State(state): State<AdminState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let usage_aggregator = match require_usage_aggregator(&state) {
        Ok(ua) => ua,
        Err(resp) => return resp,
    };
    let (window, key_id) = match stats_query_parts(&params) {
        Ok(parts) => parts,
        Err(message) => return stats_bad_request(message),
    };
    let group = parse_group_filter(&params);
    let cred_ids = group_to_cred_ids(&state, group.as_deref());
    let points = usage_aggregator.query_timeseries(window, key_id, cred_ids.as_ref());
    Json(points).into_response()
}

/// GET /api/admin/stats/by-model?range=24h|7d|30d
pub async fn stats_by_model(
    State(state): State<AdminState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let usage_aggregator = match require_usage_aggregator(&state) {
        Ok(ua) => ua,
        Err(resp) => return resp,
    };
    let (window, key_id) = match stats_query_parts(&params) {
        Ok(parts) => parts,
        Err(message) => return stats_bad_request(message),
    };
    let data = usage_aggregator.query_by_model(window, key_id);
    Json(data).into_response()
}

/// GET /api/admin/stats/by-credential?range=24h|7d|30d
pub async fn stats_by_credential(
    State(state): State<AdminState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    let usage_aggregator = match require_usage_aggregator(&state) {
        Ok(ua) => ua,
        Err(resp) => return resp,
    };
    let (window, key_id) = match stats_query_parts(&params) {
        Ok(parts) => parts,
        Err(message) => return stats_bad_request(message),
    };
    let group = parse_group_filter(&params);
    let snapshot = state.service.get_all_credentials();
    let email_map: std::collections::HashMap<u64, Option<String>> = snapshot
        .credentials
        .iter()
        .map(|c| (c.id, c.email.clone()))
        .collect();
    // group 过滤暂时返回空集（CredentialStatusItem 尚无 groups 字段）
    let cred_ids: Option<std::collections::HashSet<u64>> = group.as_deref().map(|_g| {
        std::collections::HashSet::new()
    });
    let data = usage_aggregator.query_by_credential(window, key_id, cred_ids.as_ref());
    let enriched: Vec<serde_json::Value> = data
        .into_iter()
        .map(|d| {
            let email = email_map.get(&d.credential_id).cloned().flatten();
            serde_json::json!({
                "credentialId": d.credential_id,
                "email": email,
                "calls": d.calls,
                "inputTokens": d.input_tokens,
                "outputTokens": d.output_tokens,
                "errors": d.errors,
            })
        })
        .collect();
    Json(enriched).into_response()
}

// ============ Traces ============

/// GET /api/admin/traces
/// 查询请求链路追踪记录
pub async fn list_traces(
    State(state): State<AdminState>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let trace_store = match require_trace_store(&state) {
        Ok(ts) => ts,
        Err(resp) => return resp,
    };

    // group 过滤暂时不支持（CredentialStatusItem 尚无 groups 字段）
    let credential_ids: Option<Vec<u64>> = None;

    let query = TraceQuery {
        status: params.get("status").filter(|s| !s.is_empty()).cloned(),
        error_type: params.get("errorType").filter(|s| !s.is_empty()).cloned(),
        credential_id: params
            .get("credentialId")
            .and_then(|s| s.parse::<u64>().ok()),
        key_id: params.get("keyId").and_then(|s| s.parse::<u64>().ok()),
        failed_attempt_credential_id: params
            .get("failedAttemptCredentialId")
            .and_then(|s| s.parse::<u64>().ok()),
        model: params.get("model").filter(|s| !s.is_empty()).cloned(),
        only_failed: params
            .get("onlyFailed")
            .map(|s| s == "true" || s == "1")
            .unwrap_or(false),
        credential_ids,
        limit: params
            .get("limit")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(crate::admin::trace_db::DEFAULT_QUERY_LIMIT)
            .min(1000),
        offset: params
            .get("offset")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0),
    };
    let (records, total) = trace_store.query_paged(&query);

    let snapshot = state.service.get_all_credentials();
    let email_map: HashMap<u64, Option<String>> = snapshot
        .credentials
        .iter()
        .map(|c| (c.id, c.email.clone()))
        .collect();
    let client_key_name_map: HashMap<u64, String> = state
        .client_keys
        .as_ref()
        .map(|ck| {
            ck.list()
                .into_iter()
                .map(|k| (k.id, k.name))
                .collect()
        })
        .unwrap_or_default();
    let key_label = |key_id: u64| -> String {
        client_key_name_map
            .get(&key_id)
            .cloned()
            .unwrap_or_else(|| format!("#{}", key_id))
    };

    let enriched: Vec<serde_json::Value> = records
        .into_iter()
        .map(|r| {
            let final_email = email_map.get(&r.final_credential_id).cloned().flatten();
            let key_name = key_label(r.key_id);
            let attempts: Vec<serde_json::Value> = r
                .attempts
                .iter()
                .map(|a| {
                    let email = email_map.get(&a.credential_id).cloned().flatten();
                    serde_json::json!({
                        "attempt": a.attempt,
                        "credentialId": a.credential_id,
                        "email": email,
                        "endpoint": a.endpoint,
                        "httpStatus": a.http_status,
                        "outcome": a.outcome,
                        "errorSnippet": a.error_snippet,
                        "durationMs": a.duration_ms,
                    })
                })
                .collect();
            serde_json::json!({
                "traceId": r.trace_id,
                "ts": r.ts,
                "keyId": r.key_id,
                "keySource": r.key_source,
                "keyName": key_name,
                "model": r.model,
                "isStream": r.is_stream,
                "finalStatus": r.final_status,
                "finalCredentialId": r.final_credential_id,
                "finalEmail": final_email,
                "errorType": r.error_type,
                "errorMessage": r.error_message,
                "totalAttempts": r.total_attempts,
                "durationMs": r.duration_ms,
                "interruptedAfterBytes": r.interrupted_after_bytes,
                "inputTokens": r.input_tokens,
                "outputTokens": r.output_tokens,
                "cacheCreationTokens": r.cache_creation_tokens,
                "cacheReadTokens": r.cache_read_tokens,
                "totalTokens": r.input_tokens + r.output_tokens + r.cache_creation_tokens + r.cache_read_tokens,
                "credits": r.credits,
                "firstTokenMs": r.first_token_ms,
                "attempts": attempts,
            })
        })
        .collect();
    Json(serde_json::json!({ "records": enriched, "total": total })).into_response()
}

/// GET /api/admin/traces/failure-stats
/// 按凭据聚合失败次数
pub async fn trace_failure_stats(State(state): State<AdminState>) -> impl IntoResponse {
    let trace_store = match require_trace_store(&state) {
        Ok(ts) => ts,
        Err(resp) => return resp,
    };
    let stats = trace_store.failure_stats();
    let map: std::collections::HashMap<String, serde_json::Value> = stats
        .into_iter()
        .map(|(id, s)| {
            (
                id.to_string(),
                serde_json::json!({
                    "auth": s.auth,
                    "throttle": s.throttle,
                    "other": s.other,
                }),
            )
        })
        .collect();
    Json(map).into_response()
}

// ============ 账号分组（独立实体）============

fn group_to_item(
    g: &super::groups::Group,
    state: &AdminState,
) -> super::types::GroupItem {
    let credential_count = state
        .service
        .token_manager()
        .count_credentials_with_group(&g.name);
    let client_key_count = state
        .client_keys
        .as_ref()
        .map(|ck| ck.count_with_group(&g.name))
        .unwrap_or(0);
    super::types::GroupItem {
        name: g.name.clone(),
        description: g.description.clone(),
        created_at: g.created_at.clone(),
        credential_count,
        client_key_count,
    }
}

/// GET /api/admin/groups
pub async fn list_groups(State(state): State<AdminState>) -> impl IntoResponse {
    let groups = match require_groups(&state) {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    let group_list = groups.list();
    let items: Vec<super::types::GroupItem> =
        group_list.iter().map(|g| group_to_item(g, &state)).collect();
    Json(super::types::GroupsResponse {
        total: items.len(),
        groups: items,
    })
    .into_response()
}

/// POST /api/admin/groups
pub async fn create_group(
    State(state): State<AdminState>,
    Json(payload): Json<super::types::CreateGroupRequest>,
) -> impl IntoResponse {
    let groups = match require_groups(&state) {
        Ok(g) => g,
        Err(resp) => return resp,
    };
    match groups.create(payload.name, payload.description) {
        Ok(g) => Json(group_to_item(&g, &state)).into_response(),
        Err(e) => {
            let msg = e.to_string();
            let (code, resp) = if msg.contains("已存在") {
                (
                    StatusCode::CONFLICT,
                    super::types::AdminErrorResponse::invalid_request(msg),
                )
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    super::types::AdminErrorResponse::invalid_request(msg),
                )
            };
            (code, Json(resp)).into_response()
        }
    }
}

/// PATCH /api/admin/groups/:name
/// 改名 / 改备注
pub async fn update_group(
    State(state): State<AdminState>,
    Path(name): Path<String>,
    Json(payload): Json<super::types::UpdateGroupRequest>,
) -> impl IntoResponse {
    let groups = match require_groups(&state) {
        Ok(g) => g,
        Err(resp) => return resp,
    };

    if !groups.exists(&name) {
        return (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "分组 {} 不存在",
                name
            ))),
        )
            .into_response();
    }

    // 1. 改名
    let mut current_name = name.clone();
    if let Some(new_name) = payload.new_name.as_deref() {
        let trimmed = new_name.trim();
        if !trimmed.is_empty() && trimmed != name {
            match groups.rename(&name, trimmed) {
                Ok(_) => {}
                Err(e) => {
                    let msg = e.to_string();
                    let code = if msg.contains("已存在") {
                        StatusCode::CONFLICT
                    } else {
                        StatusCode::BAD_REQUEST
                    };
                    return (
                        code,
                        Json(super::types::AdminErrorResponse::invalid_request(msg)),
                    )
                        .into_response();
                }
            }
            // 级联更新凭据的分组引用（失败时回滚分组改名）
            if let Err(e) = state
                .service
                .token_manager()
                .rename_credential_group(&name, trimmed)
            {
                let _ = groups.rename(trimmed, &name);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(super::types::AdminErrorResponse::internal_error(format!(
                        "级联更新凭据失败: {}",
                        e
                    ))),
                )
                    .into_response();
            }
            // 级联更新客户端 Key 的分组引用
            if let Some(ck) = &state.client_keys {
                ck.rename_group(&name, trimmed);
            }
            current_name = trimmed.to_string();
        }
    }

    // 2. 改备注
    if let Some(desc) = payload.description {
        let desc_opt = if desc.trim().is_empty() {
            None
        } else {
            Some(desc)
        };
        if let Err(e) = groups.update_description(&current_name, desc_opt) {
            return (
                StatusCode::BAD_REQUEST,
                Json(super::types::AdminErrorResponse::invalid_request(e.to_string())),
            )
                .into_response();
        }
    }

    let group = match groups.get(&current_name) {
        Some(g) => g,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(super::types::AdminErrorResponse::internal_error(
                    "分组在更新过程中消失，状态异常",
                )),
            )
                .into_response();
        }
    };
    Json(group_to_item(&group, &state)).into_response()
}

/// DELETE /api/admin/groups/:name?force=true
pub async fn delete_group(
    State(state): State<AdminState>,
    Path(name): Path<String>,
    Query(query): Query<super::types::DeleteGroupQuery>,
) -> impl IntoResponse {
    let groups = match require_groups(&state) {
        Ok(g) => g,
        Err(resp) => return resp,
    };

    if !groups.exists(&name) {
        return (
            StatusCode::NOT_FOUND,
            Json(super::types::AdminErrorResponse::not_found(format!(
                "分组 {} 不存在",
                name
            ))),
        )
            .into_response();
    }

    let cred_count = state
        .service
        .token_manager()
        .count_credentials_with_group(&name);
    let key_count = state
        .client_keys
        .as_ref()
        .map(|ck| ck.count_with_group(&name))
        .unwrap_or(0);

    if (cred_count > 0 || key_count > 0) && !query.force {
        return (
            StatusCode::CONFLICT,
            Json(super::types::AdminErrorResponse::invalid_request(format!(
                "分组仍被引用（凭据 {} / 客户端 Key {}），传 ?force=true 级联清理",
                cred_count, key_count
            ))),
        )
            .into_response();
    }

    if query.force {
        if let Err(e) = state
            .service
            .token_manager()
            .remove_credential_group(&name)
        {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(super::types::AdminErrorResponse::internal_error(format!(
                    "级联清理凭据失败: {}",
                    e
                ))),
            )
                .into_response();
        }
        if let Some(ck) = &state.client_keys {
            ck.clear_group(&name);
        }
    }

    groups.delete(&name);
    Json(super::types::SuccessResponse::new(format!(
        "分组 {} 已删除",
        name
    )))
    .into_response()
}
