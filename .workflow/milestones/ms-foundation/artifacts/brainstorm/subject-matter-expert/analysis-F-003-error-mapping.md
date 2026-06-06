# F-003 — Unified Error Mapping

> Role: subject-matter-expert | Related decisions: SME-03, SA-03

## Architecture

Error handling in the current project is inline within `provider.rs` and `handlers.rs` — each handler interprets upstream HTTP status codes and Kiro-specific error bodies independently. The dev-source centralizes this in `error_map.rs`, which maps Kiro error responses to Anthropic-formatted error JSON with appropriate HTTP status codes and `Retry-After` headers.

The SME analysis identifies five distinct error categories in the Kiro domain:

1. **Transient upstream errors** (502/503/504, connection resets) — currently handled in `provider.rs` with retry logic, but error responses leak Kiro-specific details to the client.
2. **Rate limiting** (429 from upstream) — currently triggers `CooldownReason::RateLimitExceeded` but returns inconsistent error formats.
3. **Authentication failures** (401/403) — triggers token refresh but error messages vary.
4. **Payload errors** (400 "Improperly formed request") — the compression pipeline may produce edge cases that trigger these; the error mapper MUST detect compression-related 400s.
5. **Model availability** (`MODEL_TEMPORARILY_UNAVAILABLE`) — triggers global circuit breaker but the client error message is not Anthropic-compatible.

Proposed module: `anthropic/error_map.rs` — a single function `map_upstream_error(status, body, context) -> AnthropicError` that produces spec-compliant Anthropic error responses.

## Interface Contract

> **Cross-Role Resolution (C-002)**: Replace standalone was_compressed param with RequestContext struct; adopt SA's two-function split (classify + to_anthropic_response).

> **Cross-Role Gap (G-001)**: Consumer "anthropic/metrics.rs" does not exist — error classification counts should be recorded via MetricsCollector::record() from handlers.rs/provider.rs, per SA's F-002 architecture.

<!-- superseded by C-002 -->
```rust
pub struct AnthropicError {
    pub status: StatusCode,
    pub error_type: String,    // "overloaded_error", "rate_limit_error", etc.
    pub message: String,
    pub retry_after: Option<u64>,
}

pub fn map_upstream_error(
    status: StatusCode,
    body: &[u8],
    was_compressed: bool,
) -> AnthropicError;
```

Consumers: `anthropic/handlers.rs`, `kiro/provider.rs` (retry decision), `anthropic/metrics.rs` (error classification).

## Constraints (RFC 2119)

- The error mapper MUST produce Anthropic Messages API-compatible error JSON (`{"type": "error", "error": {"type": "...", "message": "..."}}`).
- The error mapper MUST inject `Retry-After` headers for 429 and 529 responses.
- The error mapper MUST distinguish between retryable and non-retryable errors to inform the provider's retry logic.
- The error mapper SHOULD detect post-compression 400 errors and log diagnostic context (compression stats, original payload size).
- The error mapper MUST NOT expose Kiro-internal error details (AWS account IDs, internal service names) to the client.
- Error type strings MUST align with Anthropic's documented error types: `invalid_request_error`, `authentication_error`, `permission_error`, `not_found_error`, `rate_limit_error`, `api_error`, `overloaded_error`.

## Test Approach

- Unit tests: Map each of the five error categories to expected Anthropic error format; verify `Retry-After` injection.
- Regression: Use `tools/test_400_improperly_formed.py` scenarios as test fixtures for the 400 mapping path.
- Integration: End-to-end test with a mock upstream returning each error type; verify client receives Anthropic-compatible JSON.

## TODOs

- Catalog all inline error handling in `provider.rs` and `handlers.rs` to ensure the mapper covers every path.
- Study the Anthropic API error documentation for the complete error type taxonomy.
- Determine whether `was_compressed` context should be a flag or full `CompressionStats` for diagnostic richness.
