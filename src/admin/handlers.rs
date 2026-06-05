//! Admin API HTTP 处理器

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};

use axum::http::StatusCode;

use super::{
    middleware::AdminState,
    types::{
        AddCredentialRequest, CreatePresetRequest, ImportTokenJsonRequest, SetDisabledRequest,
        SetEndpointRequest, SetPriorityRequest, SetRegionRequest, SuccessResponse,
        UpdatePresetRequest, UpdateProxyConfigRequest,
    },
};

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

/// POST /api/admin/credentials/import-token-json
/// 批量导入 token.json
pub async fn import_token_json(
    State(state): State<AdminState>,
    Json(payload): Json<ImportTokenJsonRequest>,
) -> impl IntoResponse {
    let response = state.service.import_token_json(payload).await;
    Json(response)
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
