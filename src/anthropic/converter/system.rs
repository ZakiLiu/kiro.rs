//! 系统提示词工具函数

use serde_json::{Value, json};

use crate::anthropic::types::MessagesRequest;

/// Wrap a system prompt for history injection the same way kiro-cli does:
/// empty CONTEXT ENTRY block + a blank line + `Follow this instruction: <text>`.
/// kiro-cli 2.3.0 wire literal: `"--- CONTEXT ENTRY BEGIN ---\n--- CONTEXT
/// ENTRY END ---\n\nFollow this instruction: <text>"`.
///
/// 防御性：若 `text` 已含 `--- CONTEXT ENTRY BEGIN ---` 或 `Follow this instruction:`
/// 前缀（例如用户直接把上一轮 wire 透传，或上游 caller 已经 wrap 过），不重复包装，
/// 避免 `Follow this instruction: Follow this instruction:` 双层破坏 prefix cache key。
pub(super) fn wrap_system_for_history(text: &str) -> String {
    let trimmed = text.trim_start();
    if trimmed.starts_with("--- CONTEXT ENTRY BEGIN ---")
        || trimmed.starts_with("Follow this instruction:")
    {
        return text.to_string();
    }
    format!(
        "--- CONTEXT ENTRY BEGIN ---\n--- CONTEXT ENTRY END ---\n\nFollow this instruction: {text}"
    )
}

/// Kiro `additionalModelRequestFields` thinking schema path。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingSchemaPath {
    OutputConfig,
    Reasoning,
}

/// 模型 thinking 能力元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingConfig {
    pub schema_path: ThinkingSchemaPath,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            schema_path: ThinkingSchemaPath::OutputConfig,
            efforts: vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ],
            default_effort: Some("high".to_string()),
        }
    }
}

/// 静态模型列表使用的 Claude 4.6+ output_config schema。
pub fn output_config_thinking_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "thinking": {
                "type": "object",
                "properties": {
                    "type": { "enum": ["adaptive"] },
                    "display": { "enum": ["summarized"] }
                }
            },
            "output_config": {
                "type": "object",
                "properties": {
                    "effort": { "enum": ["low", "medium", "high", "xhigh"] }
                }
            }
        }
    })
}

/// 从 Kiro `additionalModelRequestFieldsSchema` 中提取 thinking schema path。
pub fn extract_thinking_config_from_schema(schema: &Value) -> Option<ThinkingConfig> {
    let props = schema.get("properties")?.as_object()?;

    if let Some(output_config) = props.get("output_config") {
        let efforts = output_config
            .pointer("/properties/effort/enum")
            .and_then(Value::as_array)
            .map(|values| enum_values_to_strings(values))
            .filter(|items| !items.is_empty())?;
        return Some(ThinkingConfig {
            schema_path: ThinkingSchemaPath::OutputConfig,
            efforts,
            default_effort: Some("high".to_string()),
        });
    }

    if let Some(reasoning) = props.get("reasoning") {
        let efforts = reasoning
            .pointer("/properties/effort/enum")
            .and_then(Value::as_array)
            .map(|values| enum_values_to_strings(values))
            .filter(|items| !items.is_empty())?;
        return Some(ThinkingConfig {
            schema_path: ThinkingSchemaPath::Reasoning,
            efforts,
            default_effort: Some("high".to_string()),
        });
    }

    None
}

fn enum_values_to_strings(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter_map(Value::as_str)
        .map(|s| s.to_ascii_lowercase())
        .collect()
}

/// 当前静态模型表的 thinking 能力检测。
///
/// 动态 ListAvailableModels 接入后，应直接从上游
/// `additionalModelRequestFieldsSchema` 调 `extract_thinking_config_from_schema`。
pub fn thinking_config_for_model(model: &str) -> Option<ThinkingConfig> {
    let normalized = model.to_ascii_lowercase().replace('.', "-");
    if normalized.contains("claude-sonnet-4-6")
        || normalized.contains("claude-opus-4-6")
        || normalized.contains("claude-opus-4-7")
        || normalized.contains("claude-opus-4-8")
    {
        extract_thinking_config_from_schema(&output_config_thinking_schema())
    } else {
        None
    }
}

/// 生成 legacy thinking 标签前缀。
///
/// 新路径优先走 Kiro 原生 `additionalModelRequestFields`；只要请求里带
/// thinking / reasoning_effort，就不再注入 `<thinking_mode>`，避免双通道控制。
pub(super) fn generate_thinking_prefix(req: &MessagesRequest) -> Option<String> {
    if wants_native_thinking(req) {
        return None;
    }

    if let Some(t) = &req.thinking {
        if t.thinking_type == "enabled" {
            return Some(format!(
                "<thinking_mode>enabled</thinking_mode><max_thinking_length>{}</max_thinking_length>",
                t.budget_tokens
            ));
        } else if t.thinking_type == "adaptive" {
            let raw_effort = req
                .output_config
                .as_ref()
                .map(|c| c.effort.as_str())
                .unwrap_or("high");
            // 白名单归一化：仅接受 low/medium/high，非法值回退 high
            let effort = match raw_effort {
                "low" | "medium" | "high" => raw_effort,
                _ => {
                    tracing::warn!("未知的 thinking effort 值 '{}', 回退为 'high'", raw_effort);
                    "high"
                }
            };
            return Some(format!(
                "<thinking_mode>adaptive</thinking_mode><thinking_effort>{}</thinking_effort>",
                effort
            ));
        }
    }
    None
}

fn wants_native_thinking(req: &MessagesRequest) -> bool {
    req.reasoning_effort.is_some()
        || req
            .thinking
            .as_ref()
            .map(|t| t.thinking_type != "disabled")
            .unwrap_or(false)
}

fn budget_to_effort(budget_tokens: i32) -> &'static str {
    match budget_tokens {
        0..=4000 => "low",
        4001..=16000 => "medium",
        16001..=64000 => "high",
        _ => "xhigh",
    }
}

fn normalize_requested_effort(raw: &str) -> String {
    raw.trim().to_ascii_lowercase()
}

fn clamp_effort(raw: String, config: &ThinkingConfig) -> String {
    if config.efforts.iter().any(|item| item == &raw) {
        return raw;
    }

    let fallback = config
        .efforts
        .last()
        .cloned()
        .or_else(|| config.default_effort.clone())
        .unwrap_or_else(|| "high".to_string());

    tracing::warn!(
        "未知的 thinking/reasoning effort 值 '{}', 回退为 '{}'",
        raw,
        fallback
    );
    fallback
}

/// 将客户端 thinking / reasoning_effort / output_config 参数映射为
/// Kiro `additionalModelRequestFields`。
///
/// 参考 Kiro IDE 的两种 schema path：
/// - output_config → { thinking: { type: "adaptive", display: "summarized" }, output_config: { effort } }
/// - reasoning     → { reasoning: { effort } }
pub fn build_additional_model_request_fields(
    req: &MessagesRequest,
    thinking_config: Option<&ThinkingConfig>,
) -> Option<serde_json::Value> {
    // 客户端明确关闭
    if req
        .thinking
        .as_ref()
        .map(|t| t.thinking_type == "disabled")
        .unwrap_or(false)
    {
        return None;
    }

    if !wants_native_thinking(req) {
        return None;
    };

    let derived_config;
    let config = match thinking_config {
        Some(config) => Some(config),
        None => {
            derived_config = thinking_config_for_model(&req.model);
            derived_config.as_ref()
        }
    };

    let Some(config) = config else {
        // 与 Kiro-account-manager 保持一致：没有模型 schema 元数据时，
        // 仍走 Kiro native thinking 的最小开关，避免回落到旧 XML 双通道。
        return Some(json!({
            "thinking": { "type": "adaptive" }
        }));
    };

    let raw_effort = if let Some(reasoning_effort) = &req.reasoning_effort {
        normalize_requested_effort(reasoning_effort)
    } else if let Some(thinking) = &req.thinking {
        if thinking.thinking_type == "enabled" && thinking.budget_tokens > 0 {
            budget_to_effort(thinking.budget_tokens).to_string()
        } else if thinking.thinking_type == "adaptive" {
            req.output_config
                .as_ref()
                .map(|c| normalize_requested_effort(&c.effort))
                .or_else(|| config.default_effort.clone())
                .unwrap_or_else(|| "high".to_string())
        } else {
            config
                .default_effort
                .clone()
                .unwrap_or_else(|| "high".to_string())
        }
    } else {
        config
            .default_effort
            .clone()
            .unwrap_or_else(|| "high".to_string())
    };

    let effort = clamp_effort(raw_effort, config);

    match config.schema_path {
        ThinkingSchemaPath::OutputConfig => Some(json!({
            "thinking": { "type": "adaptive", "display": "summarized" },
            "output_config": { "effort": effort }
        })),
        ThinkingSchemaPath::Reasoning => Some(json!({
            "reasoning": { "effort": effort }
        })),
    }
}

/// 检查内容是否已包含thinking标签
pub(super) fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>") || content.contains("<max_thinking_length>")
}

/// 确定聊天触发类型
/// "AUTO" 模式可能会导致 400 Bad Request 错误
pub(super) fn determine_chat_trigger_type(_req: &MessagesRequest) -> String {
    "MANUAL".to_string()
}
