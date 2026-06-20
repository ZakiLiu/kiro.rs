use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAvailableModelsResponse {
    #[serde(default)]
    pub models: Vec<UpstreamModel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamModel {
    pub model_id: String,
    #[serde(default)]
    pub model_name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub token_limits: Option<TokenLimits>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenLimits {
    #[serde(default)]
    pub max_input_tokens: Option<i64>,
}

pub(crate) fn runtime_fallback_models() -> ListAvailableModelsResponse {
    fn model(model_id: &str, model_name: &str, max_input_tokens: i64) -> UpstreamModel {
        UpstreamModel {
            model_id: model_id.to_string(),
            model_name: Some(model_name.to_string()),
            description: Some("Static fallback for runtime.kiro.dev endpoint".to_string()),
            token_limits: Some(TokenLimits {
                max_input_tokens: Some(max_input_tokens),
            }),
        }
    }

    ListAvailableModelsResponse {
        models: vec![
            model("claude-sonnet-4.6", "Claude Sonnet 4.6", 1_000_000),
            model("claude-sonnet-4.5", "Claude Sonnet 4.5", 200_000),
            model("claude-opus-4.8", "Claude Opus 4.8", 1_000_000),
            model("claude-opus-4.7", "Claude Opus 4.7", 1_000_000),
            model("claude-opus-4.6", "Claude Opus 4.6", 1_000_000),
            model("claude-opus-4.5", "Claude Opus 4.5", 200_000),
            model("claude-haiku-4.5", "Claude Haiku 4.5", 200_000),
        ],
    }
}
