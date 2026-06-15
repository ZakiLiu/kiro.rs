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

use super::client_keys::SharedClientKeyManager;
use super::groups::SharedGroupManager;
use super::service::AdminService;
use super::trace_db::SharedTraceStore;
use super::types::AdminErrorResponse;
use super::usage_stats::SharedAggregator;
use crate::common::auth;
use crate::model::config::Preset;

/// Admin API 共享状态
#[derive(Clone)]
pub struct AdminState {
    /// Admin API 密钥（运行时可修改，用 RwLock 包装）
    pub admin_api_key: Arc<RwLock<String>>,
    /// Admin 服务
    pub service: Arc<AdminService>,
    /// Prompt 预设（与 AppState 共享同一 Arc，运行时可变）
    pub presets: Arc<RwLock<Vec<Preset>>>,
    /// 客户端 Key 管理器（与 anthropic 路由共享）
    /// Optional：main.rs 未初始化时为 None
    pub client_keys: Option<SharedClientKeyManager>,
    /// 用量聚合器（与 anthropic 路由共享）
    /// Optional：main.rs 未初始化时为 None
    pub usage_aggregator: Option<SharedAggregator>,
    /// 请求链路追踪存储（与 anthropic 路由共享）
    /// Optional：main.rs 未初始化时为 None
    pub trace_store: Option<SharedTraceStore>,
    /// 账号分组注册表（持久化到 groups.json）
    /// Optional：main.rs 未初始化时为 None
    pub groups: Option<SharedGroupManager>,
}

impl AdminState {
    pub fn new(admin_api_key: impl Into<String>, service: AdminService) -> Self {
        Self {
            admin_api_key: Arc::new(RwLock::new(admin_api_key.into())),
            service: Arc::new(service),
            presets: Arc::new(RwLock::new(Vec::new())),
            client_keys: None,
            usage_aggregator: None,
            trace_store: None,
            groups: None,
        }
    }

    /// 设置 Prompt 预设（共享引用）
    pub fn with_presets(mut self, presets: Arc<RwLock<Vec<Preset>>>) -> Self {
        self.presets = presets;
        self
    }

    /// 设置客户端 Key 管理器
    pub fn with_client_keys(mut self, client_keys: SharedClientKeyManager) -> Self {
        self.client_keys = Some(client_keys);
        self
    }

    /// 设置用量聚合器
    pub fn with_usage_aggregator(mut self, aggregator: SharedAggregator) -> Self {
        self.usage_aggregator = Some(aggregator);
        self
    }

    /// 设置请求链路追踪存储
    pub fn with_trace_store(mut self, store: SharedTraceStore) -> Self {
        self.trace_store = Some(store);
        self
    }

    /// 设置账号分组管理器
    pub fn with_groups(mut self, groups: SharedGroupManager) -> Self {
        self.groups = Some(groups);
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

    let current_key = state.admin_api_key.read().clone();
    match api_key {
        Some(key) if auth::constant_time_eq(&key, &current_key) => next.run(request).await,
        _ => {
            let error = AdminErrorResponse::authentication_error();
            (StatusCode::UNAUTHORIZED, Json(error)).into_response()
        }
    }
}
