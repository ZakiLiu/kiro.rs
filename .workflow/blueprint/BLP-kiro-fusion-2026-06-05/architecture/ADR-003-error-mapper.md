---
document: architecture
session_id: BLP-kiro-fusion-2026-06-05
adr_id: ADR-003
title: Unified Error Mapper
status: proposed
created: 2026-06-05
deciders:
  - fusion-blueprint-generator
traces:
  - brainstorm/decisions.md#error-mapping
---

# ADR-003: Unified Error Mapper

## Context

Error handling in the current codebase is distributed across multiple files:

- `anthropic/handlers.rs`: Maps provider errors to HTTP responses inline.
- `anthropic/stream.rs`: Handles streaming errors with separate status code logic.
- `kiro/provider.rs`: Classifies upstream failures for retry decisions.
- `admin/error.rs`: Admin-specific error types with their own mapping.

This scattering leads to inconsistent status codes for the same upstream error,
duplicated classification logic, and no awareness of request context (e.g., whether
compression was applied — critical for diagnosing 400 errors).

The dev-source has a single-function error mapper, but it lacks structured context
and is not extensible for user-defined rules.

## Decision

We MUST introduce a centralized `ErrorMapper` module at `anthropic/error_map.rs` with:

```rust
pub struct ErrorMapper {
    builtin_rules: Vec<ErrorRule>,
    custom_rules: Vec<ErrorRule>,  // From config: error_map.custom_rules
}

pub struct RequestContext {
    pub was_compressed: bool,
    pub compression_layers: Vec<String>,  // Which layers were applied
    pub upstream_status: Option<u16>,
    pub upstream_headers: HeaderMap,
    pub credential_id: String,
}

impl ErrorMapper {
    /// Classify an upstream error into a structured ErrorClass.
    pub fn classify(&self, error: &UpstreamError, ctx: &RequestContext) -> ClassifiedError;

    /// Convert a ClassifiedError into an Anthropic-compatible JSON response.
    pub fn to_anthropic_response(&self, classified: &ClassifiedError) -> (StatusCode, Json<Value>);
}
```

**Seven error classes** (see `_index.md` Section 8):
`AuthenticationError`, `RateLimitExceeded`, `InsufficientBalance`, `ModelUnavailable`,
`UpstreamTransient`, `RequestInvalid`, `InternalError`.

**Consumers**: `handlers.rs`, `stream.rs`, and `provider.rs` all call `ErrorMapper::classify()`
instead of implementing their own mapping logic.

**Compression-aware diagnostics**: When `was_compressed = true` and the upstream returns 400,
the response body SHOULD include which compression layers were applied, aiding diagnosis of
`Improperly formed request` errors (see existing `docs/troubleshooting/`).

**Custom rules**: Users MAY define additional mapping rules in config to handle upstream-specific
error patterns not covered by builtins.

## Alternatives Considered

### (a) Per-Handler Mapping (Status Quo)

Keep error mapping inline in each handler / stream processor.

**Rejected**: Leads to inconsistent status codes and duplicated logic. Adding compression
awareness would require changes in 4+ files.

### (b) Middleware-Based Error Mapping

Use Axum middleware to intercept all responses and remap errors.

**Rejected**: Middleware operates at the HTTP layer and lacks access to `RequestContext`
(compression state, credential info). The mapping needs application-level context that
middleware cannot easily access without complex state threading.

### (c) Single Function (Dev-Source Approach)

A standalone `map_error(status, body) -> (status, body)` function.

**Rejected**: Too simple for the fusion's needs. Lacks structured classification (needed
for metrics), context awareness (needed for compression diagnostics), and extensibility
(needed for custom rules).

## Consequences

**Positive:**
- Single source of truth for all error translation.
- Compression-aware diagnostics reduce troubleshooting time for 400 errors.
- Structured `ErrorClass` feeds directly into metrics (ADR-004).
- Custom rules enable deployment-specific error handling without code changes.
- `provider.rs` retry logic uses the same classification, ensuring consistent retry decisions.

**Negative:**
- All error paths MUST thread `RequestContext` through the call chain.
- Custom rules add config complexity; invalid patterns could cause silent misclassification.

**Risks:**
- Over-classification: Too many error classes make metrics noisy. Mitigated by keeping
  the set at exactly 7 well-defined categories.
- Rule ordering: Custom rules override builtins; a broad custom pattern could shadow
  important built-in classifications. Mitigation: Custom rules are evaluated first;
  log a warning when a custom rule overrides a builtin.

## Implementation Notes

- `ErrorMapper` SHOULD be constructed once at startup and stored in `AppState`.
- `RequestContext` is built in `handlers.rs` at the start of request processing and
  passed through to `provider.rs` and `stream.rs`.
- Unit tests MUST cover all 7 error classes with and without compression context.
- Integration test: Send a compressed request that triggers upstream 400, verify the
  response body includes compression layer info.
