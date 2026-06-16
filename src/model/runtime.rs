use std::sync::Arc;

use parking_lot::RwLock;

use super::config::{Config, SystemPromptPosition};

#[derive(Debug, Clone)]
pub struct PromptRuntimeConfig {
    pub enabled: bool,
    pub enabled_presets: Vec<String>,
    pub custom_content: Option<String>,
    pub position: SystemPromptPosition,
    pub strip_system_restrictions: bool,
}

impl PromptRuntimeConfig {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            enabled: cfg.system_prompt_enabled,
            enabled_presets: cfg.enabled_presets.clone(),
            custom_content: cfg.system_prompt.clone(),
            position: cfg.system_prompt_position,
            strip_system_restrictions: cfg.strip_system_restrictions,
        }
    }

    pub fn build_injection_text(&self) -> Option<String> {
        if !self.enabled {
            return None;
        }

        let mut parts: Vec<String> = Vec::new();

        for p in crate::anthropic::prompt_presets::PRESETS {
            if self.enabled_presets.iter().any(|id| id == p.id) {
                parts.push(p.content.trim().to_string());
            }
        }

        if let Some(c) = self.custom_content.as_deref() {
            let t = c.trim();
            if !t.is_empty() {
                parts.push(t.to_string());
            }
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }
}

pub type SharedPromptConfig = Arc<RwLock<PromptRuntimeConfig>>;

pub fn shared_from_config(cfg: &Config) -> SharedPromptConfig {
    Arc::new(RwLock::new(PromptRuntimeConfig::from_config(cfg)))
}
