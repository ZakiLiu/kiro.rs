//! Gemini 兼容路由

use axum::{
    Router,
    routing::{get, post},
};

use crate::anthropic::middleware::AppState;

use super::handlers::{generate_content, list_models};

pub fn gemini_routes() -> Router<AppState> {
    Router::new()
        .route("/models", get(list_models))
        .route("/models/{*model_action}", post(generate_content))
}
