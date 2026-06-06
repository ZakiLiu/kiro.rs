# F-005 — System Prompt Presets

> Role: subject-matter-expert | Related decisions: PM-05, SME-04

## Architecture

The dev-source provides a `prompt_presets.rs` module with five built-in presets (override, pentest, nsfw, code_complete, concise) and a `prompt_filter.rs` that strips 14+ restriction patterns from system prompts. Per PM-05, the presets feature is valuable, but the filter requires independent security evaluation.

For the presets subsystem:
- `anthropic/prompt_presets.rs` — a registry of named system prompt templates, loadable from config or Admin UI.
- Each preset defines a `SystemMessage` injection strategy: prepend, append, or replace.
- The Admin UI provides a dropdown to select the active preset per-model or globally.

For the filter subsystem (deferred):
- The filter strips patterns like safety instructions and content restrictions from incoming system prompts.
- This has significant security and ethical implications — the SME position is that filter functionality SHOULD be implemented as an opt-in feature with explicit configuration, not enabled by default.

## Interface Contract

```rust
pub struct PromptPreset {
    pub name: String,
    pub description: String,
    pub system_messages: Vec<SystemMessage>,
    pub strategy: InjectionStrategy,
}

pub enum InjectionStrategy { Prepend, Append, Replace }

pub struct PresetRegistry {
    presets: HashMap<String, PromptPreset>,
    active: Option<String>,
}

impl PresetRegistry {
    pub fn apply(&self, existing_system: &mut Vec<SystemMessage>);
    pub fn set_active(&mut self, name: &str) -> Result<()>;
    pub fn list(&self) -> Vec<&PromptPreset>;
}
```

Consumers: `anthropic/handlers.rs` (system prompt injection), `admin/handlers.rs` (preset management API).

## Constraints (RFC 2119)

- Presets MUST be configurable via both `config.json` and Admin API — hardcoded presets are not acceptable for a production-oriented tool.
- Presets MUST NOT modify the system prompt in a way that breaks prefix cache keys — the preset content becomes part of the cache fingerprint.
- The prompt filter SHOULD NOT be included in the initial implementation — it requires a separate security review (see PM-05).
- Preset injection MUST preserve existing `cache_control` markers on system messages.
- Presets MAY support per-model overrides (different presets for different model families).

## Test Approach

- Unit tests: Each injection strategy (prepend, append, replace) with various system message configurations.
- Integration tests: Verify preset activation via Admin API, then confirm system prompt modification in subsequent requests.
- Cache interaction test: Verify that preset changes correctly invalidate the prefix cache (F-001).

## TODOs

- Define the initial preset library (code_complete and concise are most likely useful for the current user base).
- Design the Admin UI component for preset management — dropdown or card-based selector.
- Schedule an independent security review for the prompt filter feature before implementation.
