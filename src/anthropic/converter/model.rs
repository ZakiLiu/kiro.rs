//! 模型常量、类型定义和映射函数

use std::collections::HashMap;

use crate::anthropic::compressor::CompressionStats;
use crate::kiro::model::requests::conversation::ConversationState;

/// 单请求图片总数上限（所有消息合计，含 GIF 抽帧后的帧数）
pub(super) const MAX_TOTAL_IMAGES: usize = 20;

/// kiro-cli 2.3.0 wire-aligned system-injection ack (Q_LOG_LEVEL=trace
/// 2026-05-12 capture, history[1].assistantResponseMessage.content). Must
/// match byte-for-byte — it's part of the server's prefix-cache key.
pub(super) const SYSTEM_ACK: &str = "I will fully incorporate this information when generating my responses, and explicitly acknowledge relevant parts of the summary when answering questions.";

/// 追加到 Write 工具 description 末尾的内容
pub(super) const WRITE_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the content to write exceeds 150 lines, you MUST only write the first 50 lines using this tool, then use `Edit` tool to append the remaining content in chunks of no more than 50 lines each. If needed, leave a unique placeholder to help append content. Do NOT attempt to write all content at once.";

/// 追加到 Edit 工具 description 末尾的内容
pub(super) const EDIT_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: If the `new_string` content exceeds 50 lines, you MUST split it into multiple Edit calls, each replacing no more than 50 lines at a time. If used to append content, leave a unique placeholder to help append content. On the final chunk, do NOT include the placeholder.";

/// 追加到 Bash 工具 description 末尾的内容。
pub(super) const BASH_TOOL_DESCRIPTION_SUFFIX: &str = "- IMPORTANT: Do not send very large commands, inline scripts, or heredocs. If a command would exceed 100 lines or ~8000 characters, first create/modify a script file with chunked Write/Edit calls, then run a short command that executes it. Do not retry the same oversized command after a failure; split it smaller.";

/// 追加到系统提示词的分块写入策略
pub(super) const SYSTEM_CHUNKED_POLICY: &str = "\
When the Write or Edit tool has content size limits, always comply silently. \
Never suggest bypassing these limits via alternative tools. \
Never ask the user whether to switch approaches. \
Complete all chunked operations without commentary.";

/// Agentic 模型专用系统提示
///
/// 指导模型在 agentic 模式下的行为：持续工作、自主决策、减少确认
pub(super) const KIRO_AGENTIC_SYSTEM_PROMPT: &str = "\
You are an autonomous coding agent. Follow these principles:\n\
1. Work continuously until the task is fully complete.\n\
2. Use tools proactively without asking for permission.\n\
3. When encountering errors, debug and fix them autonomously.\n\
4. Break complex tasks into steps and execute them sequentially.\n\
5. Verify your work by reading files after writing them.\n\
6. Never ask the user for confirmation mid-task — just proceed.\n\
7. If a tool call fails, try alternative approaches before giving up.\n\
8. Prefer making changes directly over explaining what you would do.";

/// Kiro API 工具名称最大长度限制
pub(super) const TOOL_NAME_MAX_LEN: usize = 63;

/// Kiro 上游使用的规范模型 ID
pub(super) const KIRO_MODEL_SONNET_4_5: &str = "claude-sonnet-4.5";
pub(super) const KIRO_MODEL_SONNET_4_6: &str = "claude-sonnet-4.6";
pub(super) const KIRO_MODEL_SONNET_5: &str = "claude-sonnet-5";
pub(super) const KIRO_MODEL_OPUS_4_5: &str = "claude-opus-4.5";
pub(super) const KIRO_MODEL_OPUS_4_6: &str = "claude-opus-4.6";
pub(super) const KIRO_MODEL_OPUS_4_7: &str = "claude-opus-4.7";
pub(super) const KIRO_MODEL_OPUS_4_8: &str = "claude-opus-4.8";
pub(super) const KIRO_MODEL_HAIKU_4_5: &str = "claude-haiku-4.5";

fn normalize_model_name(model: &str) -> String {
    let model = model.to_lowercase();
    let model = model.strip_suffix("-thinking").unwrap_or(&model);
    let model = model.strip_suffix("-agentic").unwrap_or(model);
    model.to_string()
}

/// 默认回退模型
pub(super) const KIRO_MODEL_DEFAULT: &str = KIRO_MODEL_SONNET_4_5;

/// 模型映射：将 Anthropic / OpenAI / Gemini 模型名映射到 Kiro 模型 ID
///
/// 映射规则（按优先级）：
/// 1. `config.custom_models` 里显式声明的别名（大小写不敏感，`-thinking` 会剥离后再试一次）
/// 2. Claude 家族：按版本号精确映射
/// 3. GPT 家族：全部映射到对应 Claude 模型
/// 4. Gemini 家族：全部映射到对应 Claude 模型
/// 5. 已知合法 Kiro 模型 ID：`claude-*` 直通
/// 6. 未知但格式合法的 ID（如 `glm-5`、`minimax-m2.5`）：原样透传给上游
/// 7. 其它（含非法字符 / 空）：返回 None
///
/// `-thinking` / `-agentic` 后缀会被剥离后再映射（除非命中显式 custom 条目）。
pub fn map_model(model: &str) -> Option<String> {
    // 1) customModels 优先（含 -thinking 剥离回退，由 lookup 内部处理）
    if let Some(cm) = crate::model::custom_models::lookup(model) {
        return Some(cm.backend_id.clone());
    }

    let normalized_model = normalize_model_name(model);

    // Claude 家族
    if normalized_model.contains("sonnet") {
        if normalized_model.contains("sonnet-5") || normalized_model.contains("sonnet-5-") {
            return Some(KIRO_MODEL_SONNET_5.to_string());
        }
        if normalized_model.contains("4-6") || normalized_model.contains("4.6") {
            return Some(KIRO_MODEL_SONNET_4_6.to_string());
        }
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if normalized_model.contains("opus") {
        if normalized_model.contains("4-5") || normalized_model.contains("4.5") {
            return Some(KIRO_MODEL_OPUS_4_5.to_string());
        } else if normalized_model.contains("4-7") || normalized_model.contains("4.7") {
            return Some(KIRO_MODEL_OPUS_4_7.to_string());
        } else if normalized_model.contains("4-8") || normalized_model.contains("4.8") {
            return Some(KIRO_MODEL_OPUS_4_8.to_string());
        }
        return Some(KIRO_MODEL_OPUS_4_6.to_string());
    }
    if normalized_model.contains("haiku") {
        return Some(KIRO_MODEL_HAIKU_4_5.to_string());
    }

    // GPT 家族 → Claude 映射
    if normalized_model.starts_with("gpt-5.5") || normalized_model.starts_with("gpt-5-5") {
        return Some(KIRO_MODEL_OPUS_4_6.to_string());
    }
    if normalized_model.starts_with("gpt-5") {
        return Some(KIRO_MODEL_OPUS_4_6.to_string());
    }
    if normalized_model.starts_with("gpt-4.5") || normalized_model.starts_with("gpt-4-5") {
        return Some(KIRO_MODEL_SONNET_4_6.to_string());
    }
    if normalized_model.starts_with("gpt-4o") || normalized_model == "gpt-4-turbo" {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if normalized_model.starts_with("gpt-4") {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if normalized_model.starts_with("gpt-3.5") || normalized_model.starts_with("gpt-3-5") {
        return Some(KIRO_MODEL_HAIKU_4_5.to_string());
    }
    if normalized_model.starts_with("o4-mini")
        || normalized_model.starts_with("o3-mini")
        || normalized_model.starts_with("o1-mini")
    {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if normalized_model.starts_with("o1")
        || normalized_model.starts_with("o3")
        || normalized_model.starts_with("o4")
    {
        return Some(KIRO_MODEL_OPUS_4_6.to_string());
    }

    // Fable 家族 → Claude 映射
    if normalized_model.contains("fable-5") || normalized_model.contains("fable5") {
        return Some(KIRO_MODEL_SONNET_4_6.to_string());
    }

    // Gemini 家族 → Claude 映射（strip models/ prefix for Gemini API paths）
    let gemini_model = normalized_model
        .strip_prefix("models/")
        .unwrap_or(&normalized_model);
    if gemini_model.contains("gemini-3") && gemini_model.contains("pro") {
        return Some(KIRO_MODEL_OPUS_4_6.to_string());
    }
    if gemini_model.contains("gemini-3") && gemini_model.contains("flash") {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if gemini_model.contains("gemini-2.5-pro") || gemini_model.contains("gemini-2-5-pro") {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if gemini_model.contains("gemini-2.5-flash") || gemini_model.contains("gemini-2-5-flash") {
        return Some(KIRO_MODEL_HAIKU_4_5.to_string());
    }
    if gemini_model.contains("gemini-2") && gemini_model.contains("pro") {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if gemini_model.contains("gemini-1.5-pro") || gemini_model.contains("gemini-1-5-pro") {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }
    if gemini_model.contains("gemini-1.5-flash") || gemini_model.contains("gemini-1-5-flash") {
        return Some(KIRO_MODEL_HAIKU_4_5.to_string());
    }
    if gemini_model.contains("gemini") {
        return Some(KIRO_MODEL_SONNET_4_5.to_string());
    }

    // 特殊模型
    if normalized_model == "simple-task" {
        return Some(KIRO_MODEL_HAIKU_4_5.to_string());
    }
    if normalized_model == "auto" || normalized_model == "default" {
        return Some(KIRO_MODEL_DEFAULT.to_string());
    }

    // 直通：如果已经是有效的 Kiro 模型 ID，直接透传
    if normalized_model.starts_with("claude-") {
        return Some(normalized_model);
    }

    // 未知但格式合法的 ID 原样透传：交由上游决定可用性（避免 kiro 上新模型时被前端拦截）。
    // 合法字符集：`[a-zA-Z0-9._\-]`；空字符串 / 含空格 / 含非法字符则返回 None。
    if is_passthrough_safe(&normalized_model) {
        return Some(normalized_model);
    }

    None
}

/// 判断模型 ID 是否可以原样透传给上游。
///
/// 允许：`a-z`、`A-Z`、`0-9`、`.`、`_`、`-`；不允许空字符串。
fn is_passthrough_safe(model: &str) -> bool {
    if model.is_empty() {
        return false;
    }
    model
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// 判断模型名是否为 agentic 变体
pub fn is_agentic_model(model: &str) -> bool {
    model.to_lowercase().ends_with("-agentic")
}

/// 转换结果
#[derive(Debug)]
pub struct ConversionResult {
    /// 转换后的 Kiro 请求
    pub conversation_state: ConversationState,
    /// 压缩统计信息（仅在启用压缩时有值）
    pub compression_stats: Option<CompressionStats>,
    /// 工具名称映射（短名称 → 原始名称），仅当存在超长工具名时非空
    pub tool_name_map: HashMap<String, String>,
}

/// 转换错误
#[derive(Debug)]
pub enum ConversionError {
    #[allow(dead_code)]
    UnsupportedModel(String),
    EmptyMessages,
    EmptyMessageContent,
    UnsupportedToolMapping(String),
}

impl std::fmt::Display for ConversionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConversionError::UnsupportedModel(model) => write!(f, "模型不支持: {}", model),
            ConversionError::EmptyMessages => write!(f, "消息列表为空"),
            ConversionError::EmptyMessageContent => write!(f, "消息内容为空"),
            ConversionError::UnsupportedToolMapping(reason) => {
                write!(f, "工具映射不支持: {}", reason)
            }
        }
    }
}

impl std::error::Error for ConversionError {}

#[cfg(test)]
mod passthrough_tests {
    use super::*;

    #[test]
    fn passthrough_allows_known_open_ids() {
        assert_eq!(map_model("glm-5"), Some("glm-5".to_string()));
        assert_eq!(map_model("minimax-m2.5"), Some("minimax-m2.5".to_string()));
        assert_eq!(map_model("deepseek-3.2"), Some("deepseek-3.2".to_string()));
    }

    #[test]
    fn passthrough_rejects_illegal_ids() {
        assert_eq!(map_model(""), None);
        assert_eq!(map_model("has space"), None);
        assert_eq!(map_model("bad$char"), None);
    }

    #[test]
    fn passthrough_keeps_claude_direct() {
        assert_eq!(
            map_model("claude-opus-4.6"),
            Some("claude-opus-4.6".to_string())
        );
    }
}
