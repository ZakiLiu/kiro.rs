# Context: Phase 01 — Features & Refactoring (Milestone 3)

**Date**: 2026-06-05
**Scope**: micro (Phase 1 of ms-capability)
**Areas**: Prompt presets, PDF support, converter/stream/token_manager module split

## Constraints

### Locked

1. **Prompt presets WITHOUT filter** — Presets are configurable system prompt library. Prompt filter (restriction stripping) is DEFERRED pending security review. (Source: brainstorm PM-05)

2. **Preset injection order** — Preset system_prompt prepended BEFORE compression, so compressed requests include preset content. Selection via `x-preset-id` header. (Source: EPIC-003 S-010 AC)

3. **PDF feature-gated** — lopdf dependency MUST be feature-gated in Cargo.toml. Extraction failure MUST NOT cause request failure — graceful fallback to placeholder text. (Source: EPIC-003 S-012 AC, brainstorm SME-09)

4. **PDF size limits** — Max 10MB raw PDF, max 200K chars extracted. (Source: blueprint REQ-006)

5. **Module refactoring: zero functional change** — Each split MUST produce identical `cargo test` results. One commit per split. No downstream import changes (re-exports via mod.rs). (Source: EPIC-004 safety protocol)

6. **Refactoring split dimensions** — converter.rs → 6 sub-modules (mod, model_map, tool_convert, content, message, schema). stream.rs → 5 sub-modules (mod, event_map, usage, sse_format, context). token_manager.rs → 5 sub-modules (mod, refresh, selection, balance, types). (Source: EPIC-004 stories)

7. **CLI endpoint preserved** — MUST NOT modify kiro/endpoint/cli.rs or kiro/endpoint/ide.rs during any refactoring. (Source: user constraint SA-11)

### Free

8. **Preset storage format** — JSON in config.json (recommended) or separate presets.json file. Implementer's choice.

9. **lopdf vs pdf-extract** — Implementer MAY choose alternative PDF crate if lopdf has quality issues. (Source: EPIC-003 risks)

10. **Admin UI for presets** — Implementer MAY implement as API-only initially, defer UI to future iteration. (Source: practical — Admin UI is React frontend, separate build)

11. **Refactoring order** — Implementer MAY split in any order. Recommended: converter first (most touched by features), then stream, then token_manager.

### Deferred

12. **Prompt filter** — prompt_filter.rs from dev-source. DEFERRED indefinitely pending security/TOS review. (Source: brainstorm PM-05, blueprint W-001)

13. **Admin UI preset panel** — Full React UI for preset management. Deferred — API endpoints sufficient for now.

## Code Context

### Prompt Presets Integration Points
- Config: `src/model/config.rs` — add presets array
- Converter: `src/anthropic/converter.rs` — inject system prompt before conversion
- Handlers: `src/anthropic/handlers.rs` — read x-preset-id header, lookup preset
- AppState: `src/anthropic/middleware.rs` — store presets

### PDF Integration Points
- Converter: `src/anthropic/converter.rs` — detect document blocks with media_type=application/pdf
- New module: `src/anthropic/pdf.rs` or inline in converter content processing
- Cargo.toml: add `lopdf = "0.32"` under `[dependencies]` with optional feature gate

### Module Split Targets
- `src/anthropic/converter.rs` → `src/anthropic/converter/` (6 sub-modules)
- `src/anthropic/stream.rs` → `src/anthropic/stream/` (5 sub-modules)
- `src/kiro/token_manager.rs` → `src/kiro/token_manager/` (5 sub-modules)
