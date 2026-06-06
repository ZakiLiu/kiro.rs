# F-005 -- System Prompt Presets

> Role: system-architect | Related decisions: SA-09, PM-05, SME-03

## Architecture

A new module `src/anthropic/prompt_presets.rs` provides a configurable library of system prompt templates. Dev-source implements five built-in presets (override, pentest, nsfw, code_complete, concise) plus a prompt filter (14+ regex patterns for restriction stripping). Per PM-05, the presets feature is adopted; the filter feature requires independent security evaluation.

**Module placement:** `src/anthropic/prompt_presets.rs` within the anthropic module since presets modify the Anthropic-format system prompt before conversion.

**Storage:** Presets are defined in config.json under a `promptPresets` array. Each preset has a name, description, and system_messages array. The active preset is selected per-request via a custom header (`X-Prompt-Preset`) or globally via config.

**Integration flow:**
1. Config loads presets at startup into a PromptPresetStore
2. On request, handler checks for X-Prompt-Preset header
3. If preset specified, converter prepends/replaces system messages before Kiro conversion
4. Admin API provides CRUD for presets (list, get, create, update, delete)
5. Preset changes via Admin API update the in-memory store and optionally persist to config.json

**Prompt Filter (deferred):** The dev-source prompt_filter.rs strips restrictive instructions from system prompts using 14+ regex patterns. This is architecturally separable from presets. Per PM-05, filter evaluation is deferred pending security impact analysis. The module structure SHOULD accommodate a future `prompt_filter.rs` sibling module.

## Interface Contract

```rust
pub struct PromptPresetStore {
    presets: parking_lot::RwLock<Vec<PromptPreset>>,
}

pub struct PromptPreset {
    pub name: String,
    pub description: String,
    pub system_messages: Vec<SystemMessage>,
    pub mode: PresetMode,  // Prepend | Replace | Append
}

pub enum PresetMode {
    Prepend,   // add before existing system messages
    Replace,   // replace all system messages
    Append,    // add after existing system messages
}

impl PromptPresetStore {
    pub fn from_config(presets: Vec<PromptPreset>) -> Self;
    pub fn get(&self, name: &str) -> Option<PromptPreset>;
    pub fn list(&self) -> Vec<PresetMeta>;
    pub fn add(&self, preset: PromptPreset) -> Result<()>;
    pub fn remove(&self, name: &str) -> Result<()>;
}
```

## Constraints (RFC 2119)

- SHOULD store presets in config.json for persistence across restarts
- MUST NOT allow preset modification without admin authorization (Admin API key required)
- MUST support at most 20 presets to bound memory usage
- SHOULD support three preset modes: Prepend, Replace, Append
- MUST validate preset names are unique and non-empty
- MUST NOT introduce the prompt filter feature without explicit security review (per PM-05)
- MAY support per-request preset selection via custom header

## Test Approach

- **Unit tests:** Preset store CRUD operations. Preset application in each mode (Prepend, Replace, Append). Config serialization/deserialization round-trip.
- **Integration tests:** Request with X-Prompt-Preset header applies correct system messages. Admin API preset management. Preset persistence across config reload.
- **Security tests:** Verify presets cannot bypass authentication. Verify preset content is sanitized (no injection of control characters).
- **Edge cases:** Empty preset name. Preset with empty system_messages. More than 20 presets (rejection). Concurrent preset modification.

## TODOs

- Define the exact config.json schema for promptPresets array
- Evaluate whether preset selection should be per-API-key or global
- Design the Admin UI integration for preset management (out of scope for system-architect, defer to PM)
- Prepare architectural hooks for future prompt_filter.rs integration
- Determine whether presets interact with compression (system messages added by presets increase request size)
