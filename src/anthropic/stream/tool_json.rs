//! 工具调用 JSON 累积器
//!
//! 上游 ToolUseEvent 分片传输工具输入 JSON，本模块将分片累积为完整 JSON 后输出。
//! 检测不完整/无效 JSON 并报告结构化错误，防止半截 input_json_delta 触发客户端工具执行失败。

use std::collections::HashMap;

use serde_json::Value;

use crate::kiro::model::events::ToolUseEvent;

#[derive(Debug, Clone)]
pub struct CompletedToolUse {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone)]
pub enum ToolJsonAccumulatorError {
    InvalidJson {
        tool_use_id: String,
        name: String,
        message: String,
    },
    IncompleteJson {
        tool_use_id: String,
        name: String,
        bytes: usize,
    },
}

impl ToolJsonAccumulatorError {
    #[allow(dead_code)]
    pub fn error_type(&self) -> &'static str {
        "upstream_tool_json_error"
    }

    pub fn message(&self) -> String {
        match self {
            Self::InvalidJson {
                tool_use_id,
                name,
                message,
            } => format!(
                "Upstream returned invalid JSON for tool_use {} ({}): {}",
                tool_use_id, name, message
            ),
            Self::IncompleteJson {
                tool_use_id,
                name,
                bytes,
            } => format!(
                "Upstream ended before completing tool_use {} ({}) JSON input; buffered {} bytes.",
                tool_use_id, name, bytes
            ),
        }
    }
}

impl std::fmt::Display for ToolJsonAccumulatorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for ToolJsonAccumulatorError {}

#[derive(Debug, Default)]
pub struct ToolJsonAccumulator {
    buffers: HashMap<String, (String, String)>,
}

impl ToolJsonAccumulator {
    pub fn new() -> Self {
        Self {
            buffers: HashMap::new(),
        }
    }

    pub fn push(
        &mut self,
        tool_use: &ToolUseEvent,
        tool_name_map: &HashMap<String, String>,
    ) -> Result<Option<CompletedToolUse>, ToolJsonAccumulatorError> {
        let entry = self
            .buffers
            .entry(tool_use.tool_use_id.clone())
            .or_insert_with(|| (tool_use.name.clone(), String::new()));
        if entry.0.is_empty() {
            entry.0.clone_from(&tool_use.name);
        }
        entry.1.push_str(&tool_use.input);

        if !tool_use.stop {
            return Ok(None);
        }

        let (kiro_name, input_json) = self
            .buffers
            .remove(&tool_use.tool_use_id)
            .unwrap_or_else(|| (tool_use.name.clone(), tool_use.input.clone()));
        let input = if input_json.trim().is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str::<Value>(&input_json).map_err(|e| {
                ToolJsonAccumulatorError::InvalidJson {
                    tool_use_id: tool_use.tool_use_id.clone(),
                    name: kiro_name.clone(),
                    message: e.to_string(),
                }
            })?
        };

        let (name, input) = crate::anthropic::converter::restore_tool_use_for_client(
            &kiro_name,
            input,
            tool_name_map,
        );

        Ok(Some(CompletedToolUse {
            id: tool_use.tool_use_id.clone(),
            name,
            input,
        }))
    }

    pub fn finish(&mut self) -> Result<(), ToolJsonAccumulatorError> {
        if let Some((tool_use_id, (name, input))) = self
            .buffers
            .iter()
            .max_by_key(|(_, (_, input))| input.len())
            .map(|(id, (name, input))| (id.clone(), (name.clone(), input.clone())))
        {
            self.buffers.remove(&tool_use_id);
            return Err(ToolJsonAccumulatorError::IncompleteJson {
                tool_use_id,
                name,
                bytes: input.len(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool_event(id: &str, name: &str, input: &str, stop: bool) -> ToolUseEvent {
        ToolUseEvent {
            tool_use_id: id.to_string(),
            name: name.to_string(),
            input: input.to_string(),
            stop,
        }
    }

    #[test]
    fn complete_tool_use_returns_parsed_json() {
        let mut acc = ToolJsonAccumulator::new();
        let map = HashMap::new();
        let ev = make_tool_event("t1", "web_search", r#"{"query":"rust"}"#, true);
        let result = acc.push(&ev, &map).unwrap();
        assert!(result.is_some());
        let completed = result.unwrap();
        assert_eq!(completed.id, "t1");
        assert_eq!(completed.name, "web_search");
        assert_eq!(completed.input["query"], "rust");
    }

    #[test]
    fn incremental_chunks_accumulate() {
        let mut acc = ToolJsonAccumulator::new();
        let map = HashMap::new();
        let ev1 = make_tool_event("t1", "exec", r#"{"cm"#, false);
        let ev2 = make_tool_event("t1", "exec", r#"d":"ls"}"#, true);
        assert!(acc.push(&ev1, &map).unwrap().is_none());
        let result = acc.push(&ev2, &map).unwrap().unwrap();
        assert_eq!(result.input["cmd"], "ls");
    }

    #[test]
    fn empty_input_becomes_empty_object() {
        let mut acc = ToolJsonAccumulator::new();
        let map = HashMap::new();
        let ev = make_tool_event("t1", "tool", "", true);
        let result = acc.push(&ev, &map).unwrap().unwrap();
        assert_eq!(result.input, serde_json::json!({}));
    }

    #[test]
    fn invalid_json_returns_error() {
        let mut acc = ToolJsonAccumulator::new();
        let map = HashMap::new();
        let ev = make_tool_event("t1", "tool", "{broken", true);
        let err = acc.push(&ev, &map).unwrap_err();
        assert!(matches!(err, ToolJsonAccumulatorError::InvalidJson { .. }));
        assert!(err.message().contains("t1"));
    }

    #[test]
    fn finish_detects_incomplete() {
        let mut acc = ToolJsonAccumulator::new();
        let map = HashMap::new();
        let ev = make_tool_event("t1", "tool", r#"{"pending"#, false);
        acc.push(&ev, &map).unwrap();
        let err = acc.finish().unwrap_err();
        assert!(matches!(
            err,
            ToolJsonAccumulatorError::IncompleteJson { .. }
        ));
        assert!(err.message().contains("t1"));
    }

    #[test]
    fn finish_succeeds_when_empty() {
        let mut acc = ToolJsonAccumulator::new();
        assert!(acc.finish().is_ok());
    }

    #[test]
    fn tool_name_map_restores_original_name() {
        let mut acc = ToolJsonAccumulator::new();
        let mut map = HashMap::new();
        map.insert(
            "short_name".to_string(),
            "very_long_original_tool_name".to_string(),
        );
        let ev = make_tool_event("t1", "short_name", "{}", true);
        let result = acc.push(&ev, &map).unwrap().unwrap();
        assert_eq!(result.name, "very_long_original_tool_name");
    }

    #[test]
    fn tool_name_map_restores_claude_code_input() {
        let mut acc = ToolJsonAccumulator::new();
        let mut map = HashMap::new();
        map.insert("fs_write".to_string(), "Write".to_string());
        let ev = make_tool_event(
            "t1",
            "fs_write",
            r#"{"path":"/tmp/a.txt","text":"hello"}"#,
            true,
        );
        let result = acc.push(&ev, &map).unwrap().unwrap();
        assert_eq!(result.name, "Write");
        assert_eq!(
            result.input,
            serde_json::json!({"file_path": "/tmp/a.txt", "content": "hello"})
        );
    }
}
