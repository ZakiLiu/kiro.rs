use crate::model::config::SystemPromptPosition;
use crate::model::runtime::SharedPromptConfig;

use super::types::{MessagesRequest, SystemMessage};

pub(crate) fn inject_system_prompt(payload: &mut MessagesRequest, shared: &SharedPromptConfig) {
    let (injection, position, strip_restrictions) = {
        let cfg = shared.read();
        (
            cfg.build_injection_text(),
            cfg.position,
            cfg.strip_system_restrictions,
        )
    };

    if strip_restrictions && let Some(ref mut system) = payload.system {
        for msg in system.iter_mut() {
            let stripped = super::prompt_filter::strip_restrictions(&msg.text);
            if stripped.len() != msg.text.len() {
                tracing::info!(
                    "剥离系统提示词限制: {} → {} bytes",
                    msg.text.len(),
                    stripped.len()
                );
                msg.text = stripped;
            }
        }
    }

    let Some(text) = injection else {
        return;
    };
    let injected = SystemMessage {
        text,
        block_type: None,
        cache_control: None,
    };

    match &mut payload.system {
        Some(existing) => match position {
            SystemPromptPosition::Prepend => existing.insert(0, injected),
            SystemPromptPosition::Append => existing.push(injected),
        },
        None => {
            payload.system = Some(vec![injected]);
        }
    }
}
