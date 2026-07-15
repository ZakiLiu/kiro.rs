//! 工具转换与名称管理

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::kiro::model::requests::conversation::Message;
use crate::kiro::model::requests::tool::{InputSchema, Tool as KiroTool, ToolSpecification};
use crate::model::config::ToolCompatibilityMode;

use super::model::{
    BASH_TOOL_DESCRIPTION_SUFFIX, ConversionError, EDIT_TOOL_DESCRIPTION_SUFFIX, TOOL_NAME_MAX_LEN,
    WRITE_TOOL_DESCRIPTION_SUFFIX,
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

/// Claude Code 内置工具名到 Kiro 内置工具名的映射。
fn claude_code_tool_name_to_kiro(name: &str) -> Option<&'static str> {
    match name {
        "Write" => Some("fs_write"),
        "Edit" => Some("str_replace"),
        "Bash" => Some("execute_bash"),
        "Read" => Some("read_file"),
        "Glob" => Some("file_search"),
        "Grep" => Some("grep_search"),
        "LS" => Some("list_directory"),
        "WebSearch" => Some("web_search"),
        _ => None,
    }
}

fn is_claude_code_mode(mode: ToolCompatibilityMode) -> bool {
    mode == ToolCompatibilityMode::ClaudeCode
}

/// 映射出站工具名，并记录 `Kiro name -> client name` 供响应还原。
pub(super) fn map_client_tool_name_to_kiro(
    name: &str,
    tool_name_map: &mut HashMap<String, String>,
    mode: ToolCompatibilityMode,
) -> String {
    if is_claude_code_mode(mode)
        && let Some(kiro_name) = claude_code_tool_name_to_kiro(name)
    {
        tool_name_map
            .entry(kiro_name.to_string())
            .or_insert_with(|| name.to_string());
        return kiro_name.to_string();
    }

    map_tool_name(name, tool_name_map)
}

fn optional_number(value: &serde_json::Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_u64().map(|v| v as i64))
}

fn take_first(
    object: &serde_json::Map<String, serde_json::Value>,
    keys: &[&str],
) -> Option<serde_json::Value> {
    keys.iter().find_map(|key| object.get(*key).cloned())
}

fn maybe_insert(
    output: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: Option<serde_json::Value>,
) {
    if let Some(value) = value
        && !value.is_null()
    {
        output.insert(key.to_string(), value);
    }
}

fn default_explanation(tool_name: &str) -> serde_json::Value {
    serde_json::Value::String(format!("Mapped from Claude Code {} tool.", tool_name))
}

/// 将 Claude Code 工具参数转换为 Kiro 内置工具参数。
pub(super) fn map_tool_input_to_kiro(
    client_name: &str,
    input: serde_json::Value,
    mode: ToolCompatibilityMode,
) -> Result<serde_json::Value, ConversionError> {
    if !is_claude_code_mode(mode) {
        return Ok(input);
    }
    let Some(kiro_name) = claude_code_tool_name_to_kiro(client_name) else {
        return Ok(input);
    };
    let serde_json::Value::Object(object) = input else {
        return Ok(input);
    };

    let mut output = serde_json::Map::new();
    match (client_name, kiro_name) {
        ("Write", "fs_write") => {
            maybe_insert(
                &mut output,
                "path",
                take_first(&object, &["file_path", "path"]),
            );
            maybe_insert(
                &mut output,
                "text",
                take_first(&object, &["content", "text"]),
            );
        }
        ("Edit", "str_replace") => {
            maybe_insert(
                &mut output,
                "path",
                take_first(&object, &["file_path", "path"]),
            );
            maybe_insert(
                &mut output,
                "oldStr",
                take_first(&object, &["old_string", "oldStr"]),
            );
            maybe_insert(
                &mut output,
                "newStr",
                take_first(&object, &["new_string", "newStr"]),
            );
        }
        ("Bash", "execute_bash") => {
            maybe_insert(&mut output, "command", take_first(&object, &["command"]));
            maybe_insert(&mut output, "timeout", take_first(&object, &["timeout"]));
        }
        ("Read", "read_file") => {
            if object.get("pages").is_some_and(|value| !value.is_null()) {
                return Err(ConversionError::UnsupportedToolMapping(
                    "Claude Code Read.pages has no Kiro read_file equivalent".to_string(),
                ));
            }
            maybe_insert(
                &mut output,
                "path",
                take_first(&object, &["file_path", "path"]),
            );
            let offset = object.get("offset").and_then(optional_number);
            let limit = object.get("limit").and_then(optional_number);
            if let Some(start) = offset {
                output.insert("start_line".to_string(), serde_json::json!(start));
            }
            if let Some(limit) = limit {
                let end = offset.map(|start| start + limit - 1).unwrap_or(limit);
                output.insert("end_line".to_string(), serde_json::json!(end));
            }
            maybe_insert(
                &mut output,
                "explanation",
                take_first(&object, &["explanation"]),
            );
            output
                .entry("explanation".to_string())
                .or_insert_with(|| default_explanation(client_name));
        }
        ("Glob", "file_search") => {
            maybe_insert(
                &mut output,
                "query",
                take_first(&object, &["pattern", "query"]),
            );
            maybe_insert(
                &mut output,
                "excludePattern",
                take_first(&object, &["excludePattern", "exclude"]),
            );
            if let Some(value) = take_first(&object, &["includeIgnoredFiles", "include_ignored"]) {
                output.insert(
                    "includeIgnoredFiles".to_string(),
                    match value {
                        serde_json::Value::Bool(true) => serde_json::json!("yes"),
                        serde_json::Value::Bool(false) => serde_json::json!("no"),
                        other => other,
                    },
                );
            }
            maybe_insert(
                &mut output,
                "explanation",
                take_first(&object, &["explanation"]),
            );
            output
                .entry("explanation".to_string())
                .or_insert_with(|| default_explanation(client_name));
        }
        ("Grep", "grep_search") => {
            maybe_insert(
                &mut output,
                "query",
                take_first(&object, &["pattern", "query"]),
            );
            maybe_insert(
                &mut output,
                "includePattern",
                take_first(&object, &["glob", "includePattern"]),
            );
            maybe_insert(
                &mut output,
                "excludePattern",
                take_first(&object, &["excludePattern", "exclude"]),
            );
            maybe_insert(
                &mut output,
                "caseSensitive",
                take_first(&object, &["caseSensitive", "case_sensitive"]),
            );
            maybe_insert(
                &mut output,
                "explanation",
                take_first(&object, &["explanation"]),
            );
        }
        ("LS", "list_directory") => {
            maybe_insert(&mut output, "path", take_first(&object, &["path"]));
            maybe_insert(&mut output, "depth", take_first(&object, &["depth"]));
            maybe_insert(
                &mut output,
                "explanation",
                take_first(&object, &["explanation"]),
            );
            output
                .entry("explanation".to_string())
                .or_insert_with(|| default_explanation(client_name));
        }
        ("WebSearch", "web_search") => {
            maybe_insert(&mut output, "query", take_first(&object, &["query"]));
        }
        _ => return Ok(serde_json::Value::Object(object)),
    }

    Ok(serde_json::Value::Object(output))
}

fn map_tool_input_from_kiro(kiro_name: &str, input: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(object) = input else {
        return input;
    };
    let mut output = serde_json::Map::new();

    match kiro_name {
        "fs_write" => {
            maybe_insert(
                &mut output,
                "file_path",
                take_first(&object, &["path", "file_path"]),
            );
            maybe_insert(
                &mut output,
                "content",
                take_first(&object, &["text", "content"]),
            );
        }
        "str_replace" => {
            maybe_insert(
                &mut output,
                "file_path",
                take_first(&object, &["path", "file_path"]),
            );
            maybe_insert(
                &mut output,
                "old_string",
                take_first(&object, &["oldStr", "old_string"]),
            );
            maybe_insert(
                &mut output,
                "new_string",
                take_first(&object, &["newStr", "new_string"]),
            );
        }
        "execute_bash" => {
            maybe_insert(&mut output, "command", take_first(&object, &["command"]));
            maybe_insert(&mut output, "timeout", take_first(&object, &["timeout"]));
        }
        "read_file" => {
            maybe_insert(
                &mut output,
                "file_path",
                take_first(&object, &["path", "file_path"]),
            );
            let start = object.get("start_line").and_then(optional_number);
            let end = object.get("end_line").and_then(optional_number);
            if let Some(start) = start {
                output.insert("offset".to_string(), serde_json::json!(start));
            }
            if let Some(end) = end {
                let limit = start.map(|start| end - start + 1).unwrap_or(end);
                if limit > 0 {
                    output.insert("limit".to_string(), serde_json::json!(limit));
                }
            }
        }
        "file_search" => {
            maybe_insert(
                &mut output,
                "pattern",
                take_first(&object, &["query", "pattern"]),
            );
        }
        "grep_search" => {
            maybe_insert(
                &mut output,
                "pattern",
                take_first(&object, &["query", "pattern"]),
            );
            maybe_insert(
                &mut output,
                "glob",
                take_first(&object, &["includePattern", "glob"]),
            );
            maybe_insert(
                &mut output,
                "case_sensitive",
                take_first(&object, &["caseSensitive", "case_sensitive"]),
            );
        }
        "list_directory" => {
            maybe_insert(&mut output, "path", take_first(&object, &["path"]));
        }
        "web_search" => {
            maybe_insert(&mut output, "query", take_first(&object, &["query"]));
        }
        _ => return serde_json::Value::Object(object),
    }

    serde_json::Value::Object(output)
}

/// 还原 Kiro 工具名与参数。只有出站实际记录过映射时才改写参数。
pub(crate) fn restore_tool_use_for_client(
    kiro_name: &str,
    input: serde_json::Value,
    tool_name_map: &HashMap<String, String>,
) -> (String, serde_json::Value) {
    let Some(client_name) = tool_name_map.get(kiro_name) else {
        return (kiro_name.to_string(), input);
    };

    (
        client_name.clone(),
        map_tool_input_from_kiro(kiro_name, input),
    )
}

fn optional_schema(schema: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"anyOf": [schema, {"type": "null"}]})
}

fn kiro_builtin_tool_description(kiro_name: &str, fallback: &str) -> String {
    match kiro_name {
        "fs_write" => format!(
            "Write text content to a file.\n{}",
            WRITE_TOOL_DESCRIPTION_SUFFIX
        ),
        "str_replace" => format!(
            "Replace an exact string in a file.\n{}",
            EDIT_TOOL_DESCRIPTION_SUFFIX
        ),
        "execute_bash" => format!(
            "Execute the specified bash command.\n{}",
            BASH_TOOL_DESCRIPTION_SUFFIX
        ),
        "read_file" => "Read a single file with optional line range specification.".to_string(),
        "file_search" => "Search for files by fuzzy file path query.".to_string(),
        "grep_search" => "Search file contents using a regex pattern.".to_string(),
        "list_directory" => "List directory contents.".to_string(),
        "web_search" => "Search the web for up-to-date information.".to_string(),
        _ if fallback.trim().is_empty() => kiro_name.to_string(),
        _ => fallback.to_string(),
    }
}

fn kiro_builtin_tool_schema(kiro_name: &str) -> Option<serde_json::Value> {
    Some(match kiro_name {
        "fs_write" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to file."},
                "text": {"type": "string", "description": "Contents to write into the file."}
            },
            "required": ["path", "text"],
            "additionalProperties": false
        }),
        "str_replace" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Absolute path to file."},
                "oldStr": {"type": "string", "description": "Exact string to replace."},
                "newStr": {"type": "string", "description": "Replacement string."}
            },
            "required": ["path", "oldStr", "newStr"],
            "additionalProperties": false
        }),
        "execute_bash" => serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Bash command to execute."},
                "timeout": optional_schema(serde_json::json!({"type": "number", "description": "Optional timeout in milliseconds."}))
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        "read_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to file to read."},
                "start_line": optional_schema(serde_json::json!({"type": "number", "description": "Starting line number."})),
                "end_line": optional_schema(serde_json::json!({"type": "number", "description": "Ending line number."})),
                "explanation": {"type": "string", "description": "Why this file is being read."}
            },
            "required": ["path", "explanation"],
            "additionalProperties": false
        }),
        "file_search" => serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Fuzzy filename query."},
                "explanation": {"type": "string", "description": "Why this search is being performed."},
                "excludePattern": optional_schema(serde_json::json!({"type": "string", "description": "Glob pattern for files to exclude."})),
                "includeIgnoredFiles": optional_schema(serde_json::json!({"type": "string", "description": "Whether to include ignored files, yes or no."}))
            },
            "required": ["query", "explanation"],
            "additionalProperties": false
        }),
        "grep_search" => serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "minLength": 1, "description": "Regex pattern to search for."},
                "caseSensitive": optional_schema(serde_json::json!({"type": "boolean", "description": "Whether the search should be case sensitive."})),
                "includePattern": optional_schema(serde_json::json!({"type": "string", "description": "Glob pattern for files to include."})),
                "excludePattern": optional_schema(serde_json::json!({"type": "string", "description": "Glob pattern for files to exclude."})),
                "explanation": optional_schema(serde_json::json!({"type": "string", "description": "Why this search is being performed."}))
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        "list_directory" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Path to directory."},
                "depth": optional_schema(serde_json::json!({"type": "number", "description": "Depth of recursive listing."})),
                "explanation": {"type": "string", "description": "Why this directory is being listed."}
            },
            "required": ["path", "explanation"],
            "additionalProperties": false
        }),
        "web_search" => serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query."}
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        _ => return None,
    })
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
    mode: ToolCompatibilityMode,
) -> Vec<KiroTool> {
    let Some(tools) = tools else {
        return Vec::new();
    };

    let mut seen_names = std::collections::HashSet::new();
    let mut converted = Vec::new();

    for tool in tools {
        let unsupported_web_search = tool
            .tool_type
            .as_ref()
            .is_some_and(|tool_type| tool_type.starts_with("web_search"));
        if unsupported_web_search {
            tracing::debug!(
                "过滤不支持的工具: name={}, type={:?}",
                tool.name,
                tool.tool_type
            );
            continue;
        }

        // 兼容 Anthropic、内建工具与 OpenAI function 三种格式。
        let (effective_name, effective_description, effective_schema) = if let Some(function) =
            &tool.function
            && !function.name.is_empty()
        {
            (
                function.name.clone(),
                if function.description.is_empty() {
                    tool.description.clone()
                } else {
                    function.description.clone()
                },
                if function.parameters.is_empty() {
                    serde_json::json!(tool.input_schema)
                } else {
                    serde_json::json!(function.parameters)
                },
            )
        } else if tool.name.is_empty() {
            (
                tool.tool_type
                    .as_deref()
                    .unwrap_or("unnamed_tool")
                    .to_string(),
                tool.description.clone(),
                serde_json::json!(tool.input_schema),
            )
        } else {
            (
                tool.name.clone(),
                tool.description.clone(),
                serde_json::json!(tool.input_schema),
            )
        };

        if is_claude_code_mode(mode) && effective_name == "fs_append" {
            tracing::debug!("Claude Code 兼容模式隐藏 fs_append 工具");
            continue;
        }

        let mapped_name = map_client_tool_name_to_kiro(&effective_name, tool_name_map, mode);
        if is_claude_code_mode(mode) && !seen_names.insert(mapped_name.to_lowercase()) {
            tracing::debug!("跳过重复的映射工具名: {}", mapped_name);
            continue;
        }

        let is_builtin =
            is_claude_code_mode(mode) && claude_code_tool_name_to_kiro(&effective_name).is_some();
        let mut description = if is_builtin {
            kiro_builtin_tool_description(&mapped_name, &effective_description)
        } else {
            let mut description = if effective_description.trim().is_empty() {
                format!("Tool: {}", effective_name)
            } else {
                effective_description
            };
            let suffix = match effective_name.as_str() {
                "Write" => WRITE_TOOL_DESCRIPTION_SUFFIX,
                "Edit" => EDIT_TOOL_DESCRIPTION_SUFFIX,
                "Bash" => BASH_TOOL_DESCRIPTION_SUFFIX,
                _ => "",
            };
            if !suffix.is_empty() {
                description.push('\n');
                description.push_str(suffix);
            }
            description
        };

        if max_description_chars > 0
            && let Some((index, _)) = description.char_indices().nth(max_description_chars)
        {
            description.truncate(index);
        }

        let schema = if is_builtin {
            kiro_builtin_tool_schema(&mapped_name)
                .unwrap_or_else(|| normalize_json_schema(effective_schema.clone()))
        } else {
            normalize_json_schema(effective_schema)
        };

        converted.push(KiroTool {
            tool_specification: ToolSpecification {
                name: mapped_name,
                description,
                input_schema: InputSchema::from_json(schema),
            },
        });
    }

    converted
}

/// 检查请求的工具列表中是否包含 Write 或 Edit 工具
pub(super) fn has_write_or_edit_tool(req: &MessagesRequest) -> bool {
    req.tools
        .as_ref()
        .is_some_and(|tools| tools.iter().any(|t| t.name == "Write" || t.name == "Edit"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tool(name: &str) -> AnthropicTool {
        AnthropicTool {
            tool_type: None,
            name: name.to_string(),
            description: format!("{} description", name),
            input_schema: HashMap::new(),
            max_uses: None,
            cache_control: None,
            function: None,
        }
    }

    #[test]
    fn test_claude_code_mode_maps_names_hides_append_and_deduplicates() {
        let tools = Some(vec![
            test_tool("Write"),
            test_tool("Read"),
            test_tool("fs_append"),
            test_tool("fs_write"),
        ]);
        let mut name_map = HashMap::new();
        let converted = convert_tools(&tools, 0, &mut name_map, ToolCompatibilityMode::ClaudeCode);
        let names: Vec<&str> = converted
            .iter()
            .map(|tool| tool.tool_specification.name.as_str())
            .collect();

        assert!(names.contains(&"fs_write"));
        assert!(names.contains(&"read_file"));
        assert!(!names.contains(&"fs_append"));
        assert_eq!(names.iter().filter(|name| **name == "fs_write").count(), 1);
        assert_eq!(name_map.get("read_file").map(String::as_str), Some("Read"));
    }

    #[test]
    fn test_raw_mode_preserves_tool_name_and_schema() {
        let tools = Some(vec![test_tool("Write")]);
        let mut name_map = HashMap::new();
        let converted = convert_tools(&tools, 0, &mut name_map, ToolCompatibilityMode::Raw);

        assert_eq!(converted[0].tool_specification.name, "Write");
        assert!(name_map.is_empty());
    }

    #[test]
    fn test_claude_code_write_input_round_trip() {
        let client_input = serde_json::json!({
            "file_path": "/tmp/a.txt",
            "content": "hello"
        });
        let kiro_input = map_tool_input_to_kiro(
            "Write",
            client_input.clone(),
            ToolCompatibilityMode::ClaudeCode,
        )
        .unwrap();
        assert_eq!(
            kiro_input,
            serde_json::json!({"path": "/tmp/a.txt", "text": "hello"})
        );

        let mut name_map = HashMap::new();
        name_map.insert("fs_write".to_string(), "Write".to_string());
        let (name, restored) = restore_tool_use_for_client("fs_write", kiro_input, &name_map);
        assert_eq!(name, "Write");
        assert_eq!(restored, client_input);
    }

    #[test]
    fn test_claude_code_read_pages_is_rejected() {
        let error = map_tool_input_to_kiro(
            "Read",
            serde_json::json!({"file_path": "/tmp/a.pdf", "pages": "1-3"}),
            ToolCompatibilityMode::ClaudeCode,
        )
        .unwrap_err();
        assert!(matches!(error, ConversionError::UnsupportedToolMapping(_)));
    }
}
