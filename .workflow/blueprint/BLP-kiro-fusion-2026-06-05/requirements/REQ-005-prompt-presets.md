---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-005
priority: should
wave: P2
---

# REQ-005: Configurable System Prompt Presets

## User Story

As a **Platform Operator**, I want to define a library of system prompt presets that can be selected and switched via the Admin UI, so that I can customize model behavior for different use cases without modifying client configurations.

## Description

The current kiro.rs has no runtime prompt configuration capability. Every request passes its system message through to Kiro unchanged. The dev-source project implements both `prompt_presets.rs` (configurable preset library) and `prompt_filter.rs` (system prompt stripping). Per PM-05 and SME-08, the prompt preset capability is adopted while the prompt filter is **DEFERRED** to a separate security review cycle due to TOS/detection risk.

System prompt presets allow operators to define named collections of system messages in `config.json`. When a preset is active, its system messages are prepended to (or replace, configurable) the client-provided system message. Presets are managed via the Admin API and selectable in the Admin UI. A `default` preset MAY be configured to apply automatically when no client system message is provided.

Runtime hot-reload is supported: Admin API updates to presets take effect on the next request without restarting the service. Preset storage is in-memory, sourced from `config.json` at startup and modifiable via Admin API during runtime. Changes made via Admin API are ephemeral (lost on restart) unless the operator explicitly saves the configuration.

## Acceptance Criteria

1. **Preset Definition**: Presets MUST be definable in `config.json` under a `promptPresets` key. Each preset MUST have: `name` (unique identifier), `description` (human-readable), `systemMessages` (array of system message content blocks), and `isDefault` (boolean, at most one). The system SHOULD support up to 20 presets.

2. **Admin API Management**: The Admin API MUST expose CRUD endpoints for presets: `GET /admin/presets` (list), `GET /admin/presets/:name` (get), `PUT /admin/presets/:name` (create/update), `DELETE /admin/presets/:name` (remove). Preset modifications MUST NOT require admin API key re-authentication within the same session.

3. **Runtime Application**: When a preset is active and a request arrives, the converter MUST prepend the preset's system messages to the request's existing system messages. The prepend-vs-replace behavior SHOULD be configurable per preset. The preset MUST NOT modify tool definitions, user messages, or any non-system content.

4. **Prompt Filter NOT Included**: This requirement explicitly excludes the prompt filter functionality from dev-source's `prompt_filter.rs`. Prompt filtering MUST NOT be implemented without a dedicated security and TOS review. This constraint is locked per PM-05.

## Configuration

```json
{
  "promptPresets": [
    {
      "name": "concise",
      "description": "Encourages shorter, more direct responses",
      "systemMessages": [
        { "type": "text", "text": "Be concise. Avoid unnecessary elaboration." }
      ],
      "isDefault": false,
      "mode": "prepend"
    }
  ]
}
```

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-004 | **Soft** — Converter applies presets during request transformation |
| REQ-007 | **Soft** — converter.rs split may relocate preset application logic |
| REQ-002 | **None** — No metrics integration required (MAY add preset usage tracking later) |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-09 | System Architect | Configurable presets stored in config.json, runtime switching |
| PM-05 | Product Manager | Adopt presets, defer filter to independent evaluation |
| SME-08 | Subject Matter Expert | Presets valuable; filter requires security review |
| TS-07 | Test Strategist | Malformed YAML/JSON resilience testing |

## Out of Scope (Deferred)

- **Prompt Filter** (`prompt_filter.rs` from dev-source): Strips safety instructions from system prompts. Carries non-trivial legal, TOS, and detection risk. Requires dedicated security review before adoption. See PM-05, findings-prompt-filter-risk.md.
- **Per-user preset selection**: Initial implementation uses a single active preset for all requests. Per-user or per-model preset routing is a future enhancement.
- **Preset persistence via Admin API**: Changes made via Admin API are in-memory only. Persistent save-to-disk functionality MAY be added in a future iteration.

## Admin UI Integration

The Admin UI SHOULD display:
- List of available presets with name, description, and active status
- Toggle to activate/deactivate a preset
- Simple editor for preset system messages
- Warning banner indicating that prompt filter is not available
