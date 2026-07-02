//! Anthropic API 类型定义

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// === 缓存控制 ===

/// 缓存控制配置
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub cache_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

// === 错误响应 ===

/// API 错误响应（Anthropic 格式：顶层 type + error 嵌套）
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    #[serde(rename = "type")]
    pub response_type: &'static str,
    pub error: ErrorDetail,
}

/// 错误详情
#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl ErrorResponse {
    /// 创建新的错误响应
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            response_type: "error",
            error: ErrorDetail {
                error_type: error_type.into(),
                message: message.into(),
            },
        }
    }

    /// 创建认证错误响应
    pub fn authentication_error() -> Self {
        Self::new("authentication_error", "Invalid API key")
    }
}

// === Models 端点类型 ===

/// 模型信息
#[derive(Debug, Serialize)]
pub struct Model {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub model_type: String,
    pub max_tokens: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<bool>,
    #[serde(
        rename = "additionalModelRequestFieldsSchema",
        skip_serializing_if = "Option::is_none"
    )]
    pub additional_model_request_fields_schema: Option<serde_json::Value>,
}

/// 模型列表响应
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub object: String,
    pub data: Vec<Model>,
}

// === Messages 端点类型 ===

/// 最大思考预算 tokens
const MAX_BUDGET_TOKENS: i32 = 128_000;

/// Thinking 配置
#[derive(Debug, Deserialize, Clone)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(
        default = "default_budget_tokens",
        deserialize_with = "deserialize_budget_tokens"
    )]
    pub budget_tokens: i32,
}

impl Thinking {
    /// 是否启用了 thinking（enabled 或 adaptive）
    pub fn is_enabled(&self) -> bool {
        self.thinking_type == "enabled" || self.thinking_type == "adaptive"
    }
}

fn default_budget_tokens() -> i32 {
    20000
}
fn deserialize_budget_tokens<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = i32::deserialize(deserializer)?;
    Ok(value.min(MAX_BUDGET_TOKENS))
}

/// OutputConfig 配置
#[derive(Debug, Deserialize, Clone)]
pub struct OutputConfig {
    #[serde(default = "default_effort")]
    pub effort: String,
}

fn default_effort() -> String {
    "high".to_string()
}

/// Claude Code 请求中的 metadata
#[derive(Debug, Clone, Deserialize)]
pub struct Metadata {
    /// 用户 ID，格式如: user_xxx_account__session_0b4445e1-f5be-49e1-87ce-62bbc28ad705
    pub user_id: Option<String>,
}

/// Messages 请求体
#[derive(Debug, Clone, Deserialize)]
pub struct MessagesRequest {
    pub model: String,
    /// 为 Anthropic API 兼容保留，实际不透传给 Kiro 上游
    pub max_tokens: i32,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, deserialize_with = "deserialize_system")]
    pub system: Option<Vec<SystemMessage>>,
    pub tools: Option<Vec<Tool>>,
    #[allow(dead_code)]
    pub tool_choice: Option<serde_json::Value>,
    pub thinking: Option<Thinking>,
    pub output_config: Option<OutputConfig>,
    /// OpenAI-compatible reasoning effort hint.
    ///
    /// 部分客户端会把 `reasoning_effort` 放进 Claude `/v1/messages`
    /// 兼容请求里；这里接住并映射到 Kiro native
    /// `additionalModelRequestFields`，避免调用方必须改用
    /// Anthropic `thinking` 字段。
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// Claude Code 请求中的 metadata，包含 session 信息
    pub metadata: Option<Metadata>,
}

/// 反序列化 system 字段，支持字符串或数组格式
fn deserialize_system<'de, D>(deserializer: D) -> Result<Option<Vec<SystemMessage>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // 创建一个 visitor 来处理 string 或 array
    struct SystemVisitor;

    impl<'de> serde::de::Visitor<'de> for SystemVisitor {
        type Value = Option<Vec<SystemMessage>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or an array of system messages")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(vec![SystemMessage {
                text: value.to_string(),
                block_type: None,
                cache_control: None,
            }]))
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut messages = Vec::new();
            while let Some(msg) = seq.next_element()? {
                messages.push(msg);
            }
            Ok(if messages.is_empty() {
                None
            } else {
                Some(messages)
            })
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            serde::de::Deserialize::deserialize(deserializer)
        }
    }

    deserializer.deserialize_any(SystemVisitor)
}

/// 消息
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: String,
    /// 可以是 string 或 ContentBlock 数组
    pub content: serde_json::Value,
}

impl<'de> serde::Deserialize<'de> for Message {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut raw: serde_json::Map<String, serde_json::Value> =
            serde::Deserialize::deserialize(deserializer)?;

        let role = raw
            .remove("role")
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();

        let mut content = raw.remove("content").unwrap_or(serde_json::Value::Null);

        // OpenAI 兼容：assistant + tool_calls → content 里追加 tool_use blocks
        if role == "assistant" {
            if let Some(serde_json::Value::Array(tool_calls)) = raw.remove("tool_calls") {
                if !tool_calls.is_empty() {
                    let mut blocks: Vec<serde_json::Value> = match &content {
                        serde_json::Value::Array(arr) => arr.clone(),
                        serde_json::Value::String(s) if !s.is_empty() => {
                            vec![serde_json::json!({"type": "text", "text": s})]
                        }
                        _ => Vec::new(),
                    };
                    for tc in &tool_calls {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let func = tc.get("function");
                        let name = func
                            .and_then(|f| f.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = func
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        let input: serde_json::Value =
                            serde_json::from_str(args).unwrap_or(serde_json::json!({}));
                        blocks.push(serde_json::json!({
                            "type": "tool_use", "id": id, "name": name, "input": input
                        }));
                    }
                    content = serde_json::Value::Array(blocks);
                }
            }
        }

        // OpenAI 兼容：role=tool + tool_call_id → role=user + tool_result
        let final_role;
        if role == "tool" {
            if let Some(serde_json::Value::String(tool_call_id)) = raw.remove("tool_call_id") {
                let result_text = match &content {
                    serde_json::Value::String(s) => s.clone(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                };
                content = serde_json::json!([{
                    "type": "tool_result",
                    "tool_use_id": tool_call_id,
                    "content": result_text
                }]);
                final_role = "user".to_string();
            } else {
                final_role = role;
            }
        } else {
            final_role = role;
        }

        Ok(Message {
            role: final_role,
            content,
        })
    }
}

/// 系统消息
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SystemMessage {
    pub text: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub block_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// 工具定义
///
/// 支持三种格式：
/// 1. Anthropic 工具：{ name, description, input_schema }
/// 2. WebSearch 工具：{ type: "web_search_20250305", name: "web_search", max_uses: 8 }
/// 3. OpenAI 函数：{ type: "function", function: { name, description, parameters } }
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Tool {
    /// 工具类型，如 "web_search_20250305" 或 "function"（可选）
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub tool_type: Option<String>,
    /// 工具名称
    #[serde(default)]
    pub name: String,
    /// 工具描述（普通工具必需，WebSearch 工具可选）
    #[serde(default)]
    pub description: String,
    /// 输入参数 schema（普通工具必需，WebSearch 工具无此字段）
    #[serde(default)]
    pub input_schema: HashMap<String, serde_json::Value>,
    /// 最大使用次数（仅 WebSearch 工具）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<i32>,
    /// 缓存控制
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
    /// OpenAI 函数调用格式的嵌套定义
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<OpenAIFunction>,
}

/// OpenAI 函数定义（嵌套在 Tool.function 中）
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAIFunction {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: HashMap<String, serde_json::Value>,
}

impl Tool {
    /// 检查是否为 WebSearch 工具
    #[allow(dead_code)]
    pub fn is_web_search(&self) -> bool {
        self.tool_type
            .as_ref()
            .is_some_and(|t| t.starts_with("web_search"))
    }
}

/// 内容块
#[derive(Debug, Deserialize, Serialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ImageSource>,
}

/// 图片/文档数据源
#[derive(Debug, Deserialize, Serialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default)]
    pub media_type: String,
    #[serde(default)]
    pub data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

// === Count Tokens 端点类型 ===

/// Token 计数请求
#[derive(Debug, Serialize, Deserialize)]
pub struct CountTokensRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_system"
    )]
    pub system: Option<Vec<SystemMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

/// Token 计数响应
#[derive(Debug, Serialize, Deserialize)]
pub struct CountTokensResponse {
    pub input_tokens: i32,
}

/// 根据模型名称获取上下文窗口大小
///
/// - Opus 4.6 / 4.7 / 4.8 和 Sonnet 4.6 系列: 1,000,000 tokens
/// - 其他模型: 200,000 tokens
pub fn get_context_window_size(model: &str) -> i32 {
    let model_lower = model.to_lowercase();
    let is_opus = model_lower.contains("opus");
    let is_sonnet = model_lower.contains("sonnet");
    let is_4_6 = model_lower.contains("4-6") || model_lower.contains("4.6");
    let is_4_7 = model_lower.contains("4-7") || model_lower.contains("4.7");
    let is_4_8 = model_lower.contains("4-8") || model_lower.contains("4.8");
    let is_sonnet_5 = is_sonnet && model_lower.contains("sonnet-5");
    if is_sonnet_5 || ((is_opus || is_sonnet) && is_4_6) || (is_opus && (is_4_7 || is_4_8)) {
        1_000_000
    } else {
        200_000
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_window_1m_models() {
        assert_eq!(get_context_window_size("claude-opus-4-6"), 1_000_000);
        assert_eq!(get_context_window_size("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(get_context_window_size("claude-opus-4-7"), 1_000_000);
        assert_eq!(
            get_context_window_size("claude-opus-4-7-thinking"),
            1_000_000
        );
        assert_eq!(get_context_window_size("claude-opus-4-8"), 1_000_000);
        assert_eq!(
            get_context_window_size("claude-opus-4-8-thinking"),
            1_000_000
        );
        assert_eq!(get_context_window_size("claude-opus-4.8"), 1_000_000);
    }

    #[test]
    fn test_context_window_200k_models() {
        assert_eq!(get_context_window_size("claude-opus-4-5-20251101"), 200_000);
        assert_eq!(
            get_context_window_size("claude-haiku-4-5-20251001"),
            200_000
        );
        assert_eq!(get_context_window_size("gpt-4"), 200_000);
    }
}
