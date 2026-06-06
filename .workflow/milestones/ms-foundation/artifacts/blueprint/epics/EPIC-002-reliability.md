---
document: epic
session_id: BLP-kiro-fusion-2026-06-05
epic_id: EPIC-002
title: P1 Reliability
priority: P1
mvp: false
features: [F-003, F-004]
constraints: [C-001, C-002]
---

# EPIC-002: P1 Reliability

The reliability epic unifies error handling and hardens the protocol converter. Currently, upstream Kiro errors are mapped ad-hoc across 4+ files (handler, stream, provider, cooldown); `ErrorMapper` centralizes this into a single classify-and-respond pipeline. Converter enhancements (tool name shortening) reduce upstream 400 errors caused by tool name length limits.

This epic depends on EPIC-001's CredentialIdentity for error diagnostic context and builds on the converter infrastructure that S-003 touches.

## Stories Summary

| ID | Title | Size | Trace | Dependencies |
|----|-------|------|-------|-------------|
| S-006 | ErrorMapper with classify() and to_anthropic_response() | L | F-003 | EPIC-001 complete |
| S-007 | Integrate ErrorMapper with handlers, stream, provider | M | F-003 | S-006 |
| S-008 | Tool name shortening with reversible mapping | M | F-004 | None (independent) |
| S-009 | Compression-aware error diagnostics via RequestContext | S | F-003 × synergy | S-006, S-001 |

## Story Details

### S-006: Implement ErrorMapper with classify() and to_anthropic_response()

**User Story**: As a platform operator, I want all upstream Kiro errors classified into a consistent taxonomy and mapped to proper Anthropic error responses with appropriate HTTP status codes and Retry-After headers, so that downstream clients receive predictable, actionable error information.

**Size**: L (4 pts)

**Trace**: F-003, C-002

**Acceptance Criteria**:
1. `ErrorMapper` MUST implement `classify(upstream_error) -> ErrorCategory` that maps raw Kiro errors to categories: `RateLimit`, `AuthFailure`, `ModelUnavailable`, `InvalidRequest`, `ServerError`, `NetworkTransient`
2. `ErrorMapper` MUST implement `to_anthropic_response(category, context) -> (StatusCode, ErrorBody)` producing valid Anthropic API error format (`type`, `error.type`, `error.message`)
3. `RateLimit` and `ModelUnavailable` categories MUST include `Retry-After` header with backoff hint
4. Error classification MUST be exhaustive — any unrecognized upstream error MUST map to `ServerError` with the raw message preserved in a `debug` field (only when `sensitive-logs` feature is enabled)

**Dependencies**: EPIC-001 complete (uses CredentialIdentity for diagnostic context)

---

### S-007: Integrate ErrorMapper with Handlers, Stream, and Provider

**User Story**: As a developer, I want all error paths in handlers, stream processing, and provider to use the centralized ErrorMapper, so that error formatting logic is not duplicated and error responses are consistent across all code paths.

**Size**: M (3 pts)

**Trace**: F-003

**Acceptance Criteria**:
1. `post_messages` handler MUST replace inline error formatting with `ErrorMapper::to_anthropic_response()`
2. `stream.rs` error events MUST use `ErrorMapper::classify()` to determine whether to retry or surface
3. `provider.rs` `report_failure()` MUST delegate classification to `ErrorMapper::classify()` instead of inline matching
4. Existing cooldown categories (`FailureLimit`, `InsufficientBalance`, `ModelUnavailable`, `QuotaExceeded`) MUST map 1:1 to ErrorMapper categories — no behavioral change

**Dependencies**: S-006

---

### S-008: Implement Tool Name Shortening with Reversible Mapping

**User Story**: As a power user, I want tool names automatically shortened before forwarding to upstream (which has name length limits), and restored in responses, so that clients can use long descriptive tool names without hitting upstream 400 errors.

**Size**: M (3 pts)

**Trace**: F-004

**Acceptance Criteria**:
1. Converter MUST detect tool names exceeding upstream limit (64 chars) and generate a deterministic short name (e.g., hash-based: `t_{hash8}`)
2. A `ToolNameMap` MUST be maintained per-request to reverse the mapping in response tool_use blocks
3. Short names MUST be deterministic: same input name always produces same short name within a session
4. When no tool names exceed the limit, behavior MUST be identical to current (zero overhead)

**Dependencies**: None — this is converter-internal and can be developed in parallel with S-006/S-007.

---

### S-009: Add Compression-Aware Error Diagnostics via RequestContext

**User Story**: As a developer debugging upstream 400 errors, I want error diagnostics to include whether compression was applied and what layers ran, so that I can quickly determine if compression caused a malformed request.

**Size**: S (2 pts)

**Trace**: F-003 × S-001 synergy

**Acceptance Criteria**:
1. A `RequestContext` struct MUST be threaded through the request lifecycle, accumulating: compression_applied (bool), compression_layers (Vec<&str>), original_size_bytes, compressed_size_bytes
2. When ErrorMapper classifies an `InvalidRequest` (400), the diagnostic output MUST include RequestContext data (gated behind `sensitive-logs` feature)
3. RequestContext MUST NOT add measurable latency to the happy path (<1us overhead)

**Dependencies**: S-006 (ErrorMapper), S-001 (CredentialIdentity for context tagging)

---

## Epic-Level Acceptance Criteria

1. All inline error formatting in handler, stream, and provider replaced by ErrorMapper
2. All existing error-handling tests pass without modification (behavioral parity)
3. New ErrorMapper unit tests cover all 6 error categories with at least 2 variants each
4. Tool name shortening roundtrip tested: long name → short → restored = original
5. `cargo test` green; no anti-detection or compression regressions

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| ErrorMapper migration breaks existing cooldown behavior | Medium | High | S-007 requires 1:1 mapping verification; add integration test comparing old vs new error paths |
| Tool name hash collision | Very Low | Medium | Use 8-byte hash prefix; add collision detection that falls back to sequential numbering |
| RequestContext overhead on hot path | Low | Medium | S-009 uses zero-cost abstractions; benchmark before merge |
| Concurrent development with EPIC-004 refactoring causes merge conflicts | Medium | Medium | Coordinate: EPIC-004 splits files first, EPIC-002 integrates ErrorMapper into split modules |
