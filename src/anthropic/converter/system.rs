//! 系统提示词工具函数

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

/// 生成thinking标签前缀
pub(super) fn generate_thinking_prefix(req: &MessagesRequest) -> Option<String> {
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

/// 检查内容是否已包含thinking标签
pub(super) fn has_thinking_tags(content: &str) -> bool {
    content.contains("<thinking_mode>") || content.contains("<max_thinking_length>")
}

/// 确定聊天触发类型
/// "AUTO" 模式可能会导致 400 Bad Request 错误
pub(super) fn determine_chat_trigger_type(_req: &MessagesRequest) -> String {
    "MANUAL".to_string()
}
