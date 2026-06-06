---
document: epic
session_id: BLP-kiro-fusion-2026-06-05
epic_id: EPIC-004
title: Cross-version Module Refactoring
priority: P2
mvp: false
features: [F-007]
constraints: [C-004]
---

# EPIC-004: Cross-version Module Refactoring

The refactoring epic splits three oversized modules — `converter.rs`, `stream.rs`, and `token_manager.rs` — into focused sub-modules. This is a purely structural change with zero functional modifications. Every story in this epic has a hard invariant: `cargo test` MUST pass identically before and after the split.

This epic can run in parallel with EPIC-001 and EPIC-002. However, coordinating merge order with feature epics is critical to avoid conflict storms — refactoring PRs SHOULD land in a quiet window or be rebased onto feature branches before merge.

## Stories Summary

| ID | Title | Size | Trace | Dependencies |
|----|-------|------|-------|-------------|
| S-013 | Split converter.rs into 6 sub-modules | L | F-007 | None |
| S-014 | Split stream.rs into 5 sub-modules | M | F-007 | None |
| S-015 | Split token_manager.rs into 5 sub-modules | M | F-007 | None |

## Story Details

### S-013: Split converter.rs into 6 Sub-modules

**User Story**: As a developer, I want `converter.rs` split into focused sub-modules, so that I can navigate, test, and review converter logic without scrolling through a monolithic file.

**Size**: L (4 pts)

**Trace**: F-007, C-004

**Proposed Module Structure**:
```
anthropic/converter/
  mod.rs          — public API re-exports, convert_request() orchestrator
  model_map.rs    — model name ↔ Kiro model ID mapping
  tool_convert.rs — tool schema conversion, JSON Schema normalization
  content.rs      — content block conversion (text, image, document)
  message.rs      — message array conversion, prefill handling
  schema.rs       — JSON Schema repair (required: null, properties: null)
```

**Acceptance Criteria**:
1. All public API surface from `converter.rs` MUST be re-exported via `converter/mod.rs` — no downstream import changes
2. Each sub-module MUST contain only one responsibility area; no cross-module circular dependencies
3. `cargo test` MUST produce identical results before and after the split (zero test modifications allowed)
4. `cargo clippy` MUST pass with no new warnings

**Dependencies**: None. Can start immediately.

---

### S-014: Split stream.rs into 5 Sub-modules

**User Story**: As a developer, I want `stream.rs` split into focused sub-modules, so that event parsing, SSE formatting, and usage tracking are independently testable.

**Size**: M (3 pts)

**Trace**: F-007, C-004

**Proposed Module Structure**:
```
anthropic/stream/
  mod.rs          — public API, StreamContext, main streaming loop
  event_map.rs    — Kiro event → Anthropic SSE event mapping
  usage.rs        — token usage estimation and meteringEvent handling
  sse_format.rs   — SSE line formatting and chunked encoding
  context.rs      — StreamContext struct and builder
```

**Acceptance Criteria**:
1. All public API surface from `stream.rs` MUST be re-exported via `stream/mod.rs` — no downstream import changes
2. `StreamContext` MUST remain the single orchestration point; sub-modules are pure functions or stateless helpers
3. `cargo test` MUST produce identical results before and after the split
4. `cargo clippy` MUST pass with no new warnings

**Dependencies**: None. Can start immediately. Can run in parallel with S-013.

---

### S-015: Split token_manager.rs into 5 Sub-modules

**User Story**: As a developer, I want `token_manager.rs` split into focused sub-modules, so that token refresh, credential selection, and balance caching logic are independently reviewable.

**Size**: M (3 pts)

**Trace**: F-007, C-004

**Proposed Module Structure**:
```
kiro/token_manager/
  mod.rs          — public API, TokenManager struct, get_token() orchestrator
  refresh.rs      — async token refresh logic (Social + IdC auth)
  selection.rs    — credential priority sorting and failover selection
  balance.rs      — balance cache with dynamic TTL tiers
  types.rs        — shared types (TokenEntry, RefreshResult, AuthMethod)
```

**Acceptance Criteria**:
1. All public API surface from `token_manager.rs` MUST be re-exported via `token_manager/mod.rs`
2. Internal types MUST be consolidated in `types.rs`; no type definitions scattered across sub-modules
3. `cargo test` MUST produce identical results before and after the split
4. `cargo clippy` MUST pass with no new warnings

**Dependencies**: None. Can start immediately. Can run in parallel with S-013 and S-014.

---

## Epic-Level Acceptance Criteria

1. All 3 monolithic files replaced by sub-module directories
2. Zero functional changes — behavior is bit-for-bit identical
3. `cargo test` passes with the exact same test count and results
4. `cargo clippy` passes with no new warnings
5. No downstream import path changes (all re-exports in place)

## Refactoring Safety Protocol

Each story MUST follow this sequence:

1. **Baseline**: Run `cargo test`, record output hash
2. **Split**: Move code into sub-modules with re-exports
3. **Verify**: Run `cargo test`, compare output hash — must match
4. **Lint**: Run `cargo clippy`, zero new warnings
5. **Commit**: One commit per sub-module split (not one giant commit)

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Merge conflicts with feature epics (EPIC-001, EPIC-002) | High | Medium | Land refactoring PRs in quiet windows; or rebase feature branches onto refactored structure |
| Accidental behavioral change during split | Low | High | Safety protocol above; CI gate on test hash comparison |
| Circular dependency between sub-modules | Low | Medium | Enforce DAG structure in review; `types.rs` breaks cycles by centralizing shared types |
| IDE/tooling disruption for contributors | Low | Low | Document new module structure in CLAUDE.md after merge |

## Coordination Note

If EPIC-004 stories merge before EPIC-002 stories, the ErrorMapper integration (S-007) benefits from working with smaller, focused files. If EPIC-002 merges first, EPIC-004 stories must account for ErrorMapper's additions when splitting. **Recommended**: land S-013 before S-007 to minimize conflict surface.
