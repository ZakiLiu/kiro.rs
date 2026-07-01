//! 工具转换与名称管理

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::kiro::model::requests::conversation::Message;
use crate::kiro::model::requests::tool::{InputSchema, Tool as KiroTool, ToolSpecification};

use super::model::{
    EDIT_TOOL_DESCRIPTION_SUFFIX, TOOL_NAME_MAX_LEN, WRITE_TOOL_DESCRIPTION_SUFFIX,
};
use super::schema::normalize_json_schema;
use crate::anthropic::types::{MessagesRequest, Tool as AnthropicTool};

/// 生成确定性短名称：截断前缀 + "_" + 8 位 SHA256 hex
fn shorten_tool_name(name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    let hash_suffix = &hash_hex[..8];
    let prefix_max = TOOL_NAME_MAX_LEN - 1 - 8;
    let prefix = match name.char_indices().nth(prefix_max) {
        Some((idx, _)) => &name[..idx],
        None => name,
    };
    format!("{}_{}", prefix, hash_suffix)
}

/// 如果名称超长则缩短，并记录映射（short → original）
pub(super) fn map_tool_name(name: &str, tool_name_map: &mut HashMap<String, String>) -> String {
    if name.len() <= TOOL_NAME_MAX_LEN {
        return name.to_string();
    }
    let short = shorten_tool_name(name);
    tool_name_map.insert(short.clone(), name.to_string());
    short
}

/// 收集历史消息中使用的所有工具名称（小写去重）
///
/// 返回去重后的工具名称列表，保留原始大小写（首次出现的形式），
/// 但通过小写比较避免 `read` / `Read` 这类变体重复。
pub(super) fn collect_history_tool_names(history: &[Message]) -> Vec<String> {
    let mut seen_lowercase = std::collections::HashSet::new();
    let mut tool_names = Vec::new();

    for msg in history {
        if let Message::Assistant(assistant_msg) = msg
            && let Some(ref tool_uses) = assistant_msg.assistant_response_message.tool_uses
        {
            for tool_use in tool_uses {
                if seen_lowercase.insert(tool_use.name.to_lowercase()) {
                    tool_names.push(tool_use.name.clone());
                }
            }
        }
    }

    tool_names
}

/// 为历史中使用但不在 tools 列表中的工具创建占位符定义
/// Kiro API 要求：历史消息中引用的工具必须在 currentMessage.tools 中有定义
///
/// **Round 7 (2026-05-13)**: 保持与 `normalize_json_schema` 一致 ——
/// 不主动写入 `$schema` 与 `additionalProperties` 字段（Round 6 已确认
/// kiro-cli 2.3.0 wire 不发这两个字段；强加会偏移 prefix-cache key、
/// 每工具浪费 ~80B）。
pub(super) fn create_placeholder_tool(name: &str) -> KiroTool {
    KiroTool {
        tool_specification: ToolSpecification {
            name: name.to_string(),
            description: "Tool used in conversation history".to_string(),
            input_schema: InputSchema::from_json(serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            })),
        },
    }
}

/// 转换工具定义
///
/// # 不支持的工具类型
///
/// 以下工具类型会被自动过滤（Kiro API 当前不支持）：
/// - `web_search_*`: Anthropic 的 Web 搜索工具（如 `web_search_20250305`）
///
/// **TODO**: 如果 Kiro API 未来支持 web_search，需要：
/// 1. 移除下方的 `filter` 过滤逻辑
/// 2. 添加 web_search 工具的转换逻辑（可能需要特殊处理 `max_uses` 等字段）
/// 3. 更新相关测试用例
pub(super) fn convert_tools(
    tools: &Option<Vec<AnthropicTool>>,
    max_description_chars: usize,
    tool_name_map: &mut HashMap<String, String>,
) -> Vec<KiroTool> {
    let Some(tools) = tools else {
        return Vec::new();
    };

    tools
        .iter()
        .filter(|t| {
            // 过滤掉 web_search 类型的工具（Kiro API 当前不支持）
            // 工具类型格式: "web_search_20250305"
            let dominated = t
                .tool_type
                .as_ref()
                .is_some_and(|ty| ty.starts_with("web_search"));
            if dominated {
                tracing::debug!("过滤不支持的工具: name={}, type={:?}", t.name, t.tool_type);
            }

            !dominated
        })
        .map(|t| {
            // 提取工具名称/描述/schema，兼容三种格式：
            // 1. Anthropic: { name, description, input_schema }
            // 2. 内建工具: { type: "computer_20250124" }（无 name）
            // 3. OpenAI: { type: "function", function: { name, description, parameters } }
            let (effective_name, effective_description, effective_schema) =
                if let Some(func) = &t.function
                    && !func.name.is_empty()
                {
                    (
                        func.name.clone(),
                        if func.description.is_empty() {
                            t.description.clone()
                        } else {
                            func.description.clone()
                        },
                        if func.parameters.is_empty() {
                            serde_json::json!(t.input_schema)
                        } else {
                            serde_json::json!(func.parameters)
                        },
                    )
                } else if t.name.is_empty() {
                    (
                        t.tool_type
                            .as_deref()
                            .unwrap_or("unnamed_tool")
                            .to_string(),
                        t.description.clone(),
                        serde_json::json!(t.input_schema),
                    )
                } else {
                    (
                        t.name.clone(),
                        t.description.clone(),
                        serde_json::json!(t.input_schema),
                    )
                };

            let mut description = if effective_description.trim().is_empty() {
                format!("Tool: {}", effective_name)
            } else {
                effective_description
            };

            // 对 Write/Edit 工具追加自定义描述后缀
            let suffix = match effective_name.as_str() {
                "Write" => WRITE_TOOL_DESCRIPTION_SUFFIX,
                "Edit" => EDIT_TOOL_DESCRIPTION_SUFFIX,
                _ => "",
            };
            if !suffix.is_empty() {
                description.push('\n');
                description.push_str(suffix);
            }

            // 限制描述长度（0=不截断；安全截断 UTF-8，单次遍历）
            let description = if max_description_chars > 0 {
                match description.char_indices().nth(max_description_chars) {
                    Some((idx, _)) => description[..idx].to_string(),
                    None => description,
                }
            } else {
                description
            };

            let schema = normalize_json_schema(effective_schema);

            KiroTool {
                tool_specification: ToolSpecification {
                    name: map_tool_name(&effective_name, tool_name_map),
                    description,
                    input_schema: InputSchema::from_json(schema),
                },
            }
        })
        .collect()
}

/// 检查请求的工具列表中是否包含 Write 或 Edit 工具
pub(super) fn has_write_or_edit_tool(req: &MessagesRequest) -> bool {
    req.tools
        .as_ref()
        .is_some_and(|tools| tools.iter().any(|t| t.name == "Write" || t.name == "Edit"))
}
