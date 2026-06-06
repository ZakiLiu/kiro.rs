# F-005 — System Prompt Presets

> Role: product-manager | Related decisions: PM-05

## Architecture

Dev-source implements two related but distinct prompt manipulation features:
1. **Prompt Presets** (prompt_presets.rs): A library of pre-built system prompts (override, pentest, nsfw, code_complete, concise) selectable via Admin UI at runtime.
2. **Prompt Filter** (prompt_filter.rs): System prompt restriction stripping with 14+ regex patterns to remove safety guardrails.

Per PM-05, prompt presets SHOULD be introduced as they provide genuine operational value — operators can tune model behavior for different use cases without modifying client code. The prompt filter requires independent security and legal evaluation before any adoption decision.

Product positioning: Prompt presets align with the "Professional Platform" aspect of the dual positioning (see PM-03). They enable power users to customize model behavior through a managed interface rather than client-side hacks.

## Interface Contract

- **Preset Library**: Named presets stored in config (JSON) or as embedded defaults
- **Admin API**: CRUD operations for custom presets, activation/deactivation per-credential or globally
- **Request Pipeline**: Active preset injected before user system prompt (prepend mode) or replacing it (override mode)
- **Admin UI**: Preset selector dropdown with preview of prompt content

## Constraints (RFC 2119)

- MUST implement preset library with at least: default (no modification), concise, code_complete
- MUST support Admin API for listing, activating, and deactivating presets
- MUST NOT include prompt filter (restriction stripping) in initial release; this requires separate evaluation
- SHOULD support custom user-defined presets via Admin UI
- SHOULD support per-credential preset assignment (different credentials can have different active presets)
- MAY support preset chaining (combine multiple presets in order)

## Test Approach

- Unit tests for preset injection: prepend mode and override mode produce correct final system prompt
- Unit tests for preset CRUD via Admin API
- Integration test: activate a preset, send a request, verify system prompt includes preset content
- Negative test: verify that no filter/stripping functionality is included

## TODOs

- Define the initial preset library (which presets ship as defaults)
- Evaluate prompt filter separately: document security implications, legal considerations, and user demand
- Design Admin UI preset management interface (coordinate with UI role)
- Determine if presets should be persisted to disk or only live in memory
