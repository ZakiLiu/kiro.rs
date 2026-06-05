//! Admin API 中间件

use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};

use parking_lot::RwLock;

use super::service::AdminService;
use super::types::AdminErrorResponse;
use crate::common::auth;
use crate::model::config::Preset;

/// Admin API 共享状态
#[derive(Clone)]
pub struct AdminState {
    /// Admin API 密钥
    pub admin_api_key: String,
    /// Admin 服务
    pub service: Arc<AdminService>,
    /// Prompt 预设（与 AppState 共享同一 Arc，运行时可变）
    pub presets: Arc<RwLock<Vec<Preset>>>,
}

impl AdminState {
    pub fn new(admin_api_key: impl Into<String>, service: AdminService) -> Self {
        Self {
            admin_api_key: admin_api_key.into(),
            service: Arc::new(service),
            presets: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// 设置 Prompt 预设（共享引用）
    pub fn with_presets(mut self, presets: Arc<RwLock<Vec<Preset>>>) -> Self {
        self.presets = presets;
        self
    }
}

/// Admin API 认证中间件
pub async fn admin_auth_middleware(
    State(state): State<AdminState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let api_key = auth::extract_api_key(&request);

    match api_key {
        Some(key) if auth::constant_time_eq(&key, &state.admin_api_key) => next.run(request).await,
        _ => {
            let error = AdminErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}
