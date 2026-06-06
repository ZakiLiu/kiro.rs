---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-007
priority: should
wave: cross-version
---

# REQ-007: Incremental Core Module Refactoring

## User Story

As a **developer maintaining kiro.rs**, I want the largest monolithic source files to be incrementally split into focused sub-modules, so that I can navigate, test, and modify individual concerns without cognitive overload from 500+ line files.

## Description

Three core files in kiro.rs have grown beyond comfortable maintenance size: `converter.rs` (protocol conversion), `stream.rs` (streaming response processing), and `token_manager.rs` (credential lifecycle management). The dev-source project has already performed similar splits (converter into 6 sub-modules, token_manager into 8 sub-modules), demonstrating that the decomposition is feasible.

However, the current kiro.rs module organization is actually superior in some areas — specifically the credential management split across `token_manager.rs`, `cooldown.rs`, `rate_limiter.rs`, `affinity.rs`, `fingerprint.rs`, and `background_refresh.rs` as peer files rather than nested sub-modules (see SME-02). The refactoring strategy is therefore selective: split the files that are genuinely too large, while preserving the organizational patterns that already work well.

The refactoring is **not a user-facing feature** — it is a force multiplier that makes feature work easier. Per PM-09, it SHOULD proceed incrementally alongside feature implementation (e.g., splitting converter.rs during REQ-004 work) rather than as a standalone refactoring phase. The hard constraint is TS-09: all existing 378 tests MUST pass before and after each split step.

## Acceptance Criteria

1. **Zero Regression**: Every split step MUST pass the full `cargo test` suite. No existing test may be disabled, skipped, or modified to accommodate the split. This is a hard gate per TS-09.

2. **Public API Preservation**: Module splits MUST NOT change the public API surface. Re-exports via `mod.rs` MUST maintain all existing import paths. Downstream consumers (handlers, admin, tests) MUST NOT require changes.

3. **Phase Alignment**: Splits SHOULD be driven by feature implementation needs. Converter split aligns with REQ-004 (tool shortening) and REQ-006 (PDF content blocks). Stream split aligns with REQ-001 (cache insertion) and REQ-003 (error mapping in stream). Token manager split aligns with REQ-008 (CredentialIdentity integration).

4. **Incremental Commits**: Each sub-module extraction MUST be a separate, reviewable commit. Bulk splits of entire files in a single commit are NOT acceptable.

## Target Decomposition

### converter.rs → 6 sub-modules

| Sub-module | Responsibility |
|-----------|---------------|
| `converter/mod.rs` | Re-exports, `convert_request()` orchestration |
| `converter/model_map.rs` | Model name mapping (sonnet/opus/haiku → Kiro IDs) |
| `converter/schema.rs` | JSON Schema normalization (`required: null`, `properties: null` fixes) |
| `converter/tools.rs` | Tool conversion, placeholder generation, name shortening (REQ-004) |
| `converter/content.rs` | Content block processing (text, image, PDF) |
| `converter/system.rs` | System message handling, preset application (REQ-005) |

### stream.rs → 5 sub-modules

| Sub-module | Responsibility |
|-----------|---------------|
| `stream/mod.rs` | Re-exports, `StreamContext` orchestration |
| `stream/events.rs` | Event type conversion (Kiro → Anthropic SSE) |
| `stream/usage.rs` | Usage estimation and metering event passthrough |
| `stream/tools.rs` | Tool name restoration (REQ-004 reverse mapping) |
| `stream/cache.rs` | Cache insertion on conversation_id receipt (REQ-001) |

### token_manager.rs → 5 sub-modules

| Sub-module | Responsibility |
|-----------|---------------|
| `token_manager/mod.rs` | Re-exports, `TokenManager` public API |
| `token_manager/selection.rs` | Credential selection, priority ordering, failover |
| `token_manager/refresh.rs` | Token refresh logic (Social + IdC authentication) |
| `token_manager/balance.rs` | Balance cache, dynamic TTL tiers |
| `token_manager/identity.rs` | CredentialIdentity trait implementation (REQ-008) |

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-004 | **Alignment** — converter.rs tools sub-module houses tool shortening |
| REQ-005 | **Alignment** — converter.rs system sub-module houses preset application |
| REQ-001 | **Alignment** — stream.rs cache sub-module houses cache insertion |
| REQ-008 | **Alignment** — token_manager identity sub-module houses trait impl |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-01 | System Architect | Incremental decomposition, 500 LOC threshold |
| PM-09 | Product Manager | Force multiplier, not user-facing; MUST NOT block P0 |
| SME-02 | Subject Matter Expert | Current peer-file separation is superior for credentials |
| TS-09 | Test Strategist | Zero regression gate — full test suite before and after |

## Anti-Patterns to Avoid

- **Big bang refactor**: Splitting all three files simultaneously creates merge conflicts and hides regressions.
- **Premature abstraction**: Don't introduce new traits or generics just for the split. Move code, don't redesign it.
- **Test relocation**: Tests stay with their original module unless the split makes co-location impossible. Prefer `#[cfg(test)]` in sub-modules over moving tests to `tests/`.
- **Feature coupling**: Don't combine feature implementation with refactoring in the same commit. Split first, then implement.
