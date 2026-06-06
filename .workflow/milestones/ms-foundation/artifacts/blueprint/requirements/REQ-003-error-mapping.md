---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-003
priority: must
wave: P1
---

# REQ-003: Unified Error Mapping

## User Story

As a **Platform Operator**, I want all upstream Kiro errors to be translated into well-formed Anthropic API error responses with appropriate Retry-After headers, so that downstream clients receive consistent, actionable error information regardless of the upstream error format.

## Description

Currently, error handling in kiro.rs is scattered across `provider.rs`, `handlers.rs`, `stream.rs`, and inline match arms — each with its own ad-hoc translation logic. This creates inconsistency: the same upstream error may produce different client-facing responses depending on which code path encounters it. The dev-source project solved this with a dedicated `error_map.rs` module that centralizes all error translation.

The `ErrorMapper` module provides two core functions: `classify()` which categorizes an upstream error into a well-defined `ErrorClass` enum, and `to_anthropic_response()` which produces a spec-compliant Anthropic API error JSON body. The mapper is context-aware via `RequestContext`, which carries metadata about the request lifecycle — notably `was_compressed` (to provide diagnostic hints when compression may have caused the error) and `upstream_headers` (to extract Retry-After values).

Consumers of ErrorMapper span three modules: `handlers.rs` (for synchronous errors during request validation and upstream call), `stream.rs` (for errors encountered during streaming response parsing), and `provider.rs` (for credential-level errors that trigger failover). All three MUST use ErrorMapper instead of inline error construction.

## Acceptance Criteria

1. **Classification**: ErrorMapper MUST classify all upstream HTTP status codes into one of seven `ErrorClass` variants: `RateLimit` (429), `Overloaded` (529), `BadRequest` (400), `AuthFailure` (401/403), `NotFound` (404), `ServerError` (500), `NetworkError` (502/connection errors). Every error reaching the client MUST pass through `classify()`.

2. **Anthropic Response**: `to_anthropic_response()` MUST produce a JSON body conforming to the Anthropic API error schema: `{"type": "error", "error": {"type": "<error_type>", "message": "<message>"}}`. For 429 and 529 responses, MUST inject `Retry-After` header using upstream value if present, or a calculated backoff value.

3. **RequestContext**: ErrorMapper MUST accept a `RequestContext` struct containing `was_compressed: bool` and `upstream_headers: HeaderMap`. When `was_compressed` is true and the upstream returns 400, the error message SHOULD include a diagnostic hint about potential compression-induced malformation (see SA-06).

4. **Consumer Migration**: All inline error translation in `handlers.rs`, `stream.rs`, and `provider.rs` MUST be replaced with ErrorMapper calls. No raw Kiro error format SHALL reach downstream clients.

## Error Classification Table

| Upstream Status | ErrorClass | Anthropic Status | Retryable | Retry-After |
|----------------|-----------|-----------------|-----------|-------------|
| 429 | RateLimit | 429 | Yes | From upstream or 30s default |
| 529 | Overloaded | 529 | Yes | Exponential backoff |
| 400 | BadRequest | 400 | No | None |
| 401, 403 | AuthFailure | 401 | No (credential-level) | None |
| 402 | InsufficientBalance | 400 | No (credential-level) | None |
| 404 | NotFound | 404 | No | None |
| 500 | ServerError | 500 | Yes (limited) | None |
| 502, connection reset | NetworkError | 502 | Yes | None |

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-002 | **Soft** — Error classification feeds metrics error_class_count |
| REQ-001 | **Soft** — Cache invalidation triggered by credential-level errors |
| REQ-008 | **None** — No direct dependency |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-03 | System Architect | Dedicated error_map module design |
| SA-06 | System Architect | Error mapping runs after compression |
| PM-08 | Product Manager | Directly improves downstream client experience |
| SME-06 | Subject Matter Expert | Five distinct Kiro error categories |
| SME-01 (S-001) | Cross-Role Synergy | Compression-induced 400s detected via RequestContext.was_compressed |
| TS-05 | Test Strategist | All upstream status codes must be covered |

## Interface Contract

```rust
pub enum ErrorClass {
    RateLimit,
    Overloaded,
    BadRequest,
    AuthFailure,
    InsufficientBalance,
    NotFound,
    ServerError,
    NetworkError,
}

pub struct RequestContext {
    pub was_compressed: bool,
    pub upstream_headers: HeaderMap,
    pub credential_id: Option<u64>,
}

impl ErrorMapper {
    pub fn classify(status: u16, body: &[u8]) -> ErrorClass;
    pub fn to_anthropic_response(
        class: ErrorClass,
        body: &[u8],
        ctx: &RequestContext,
    ) -> (StatusCode, HeaderMap, serde_json::Value);
}
```

## Interaction with Existing Retry Logic

ErrorMapper classification feeds into the existing credential failover logic. Retryable errors (RateLimit, Overloaded, ServerError, NetworkError) trigger `report_failure()` on the current credential and retry with the next available credential. Non-retryable errors (BadRequest, AuthFailure, NotFound) return immediately to the client. This behavior MUST NOT change — ErrorMapper classifies, the provider decides retry strategy.
