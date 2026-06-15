//! OpenAI 兼容路由

use axum::{Router, routing::post};

use crate::anthropic::middleware::AppState;

use super::handlers::{post_chat_completions, post_responses};

pub fn openai_routes() -> Router<AppState> {
    Router::new()
        .route("/chat/completions", post(post_chat_completions))
        .route("/responses", post(post_responses))
}
