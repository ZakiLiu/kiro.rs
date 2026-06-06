# F-003 -- Unified Error Mapping

> Role: system-architect | Related decisions: SA-03, SA-06, PM-01, PM-04

## Architecture

> **Cross-Role Synergy (S-001)**: Aligns with SME "Compression-induced 400 errors" pitfall — BadRequest classification should detect post-compression 400s via RequestContext.was_compressed and log diagnostic context.

A new module `src/anthropic/error_map.rs` centralizes upstream Kiro error-to-Anthropic error translation. Currently, error handling is scattered across handlers.rs (HTTP status mapping), stream.rs (streaming error events), and provider.rs (retry/failover decisions). The error_map module consolidates these into a single authoritative mapping.

**Module placement:** `src/anthropic/error_map.rs` within the anthropic module since the output format is Anthropic-specific. The module is stateless -- it provides pure functions that map (upstream_status, error_body) to (anthropic_status, anthropic_error_body, retry_info).

**Integration flow:**
1. `kiro/provider.rs` receives upstream HTTP response with status and body
2. On non-2xx status, provider calls `ErrorMapper::classify(status, &body)` to get ErrorClass
3. ErrorClass determines retry/failover behavior (retryable triggers failover per SA-05)
4. If no retry succeeds, `ErrorMapper::to_anthropic_response(status, &body)` produces the final client-facing error
5. For streaming errors (error events mid-stream), stream.rs calls the same mapper
6. Error mapping runs AFTER compression (per SA-06) -- the compressed request has already been sent

**Relationship to cooldown:** The ErrorMapper classification feeds into the existing cooldown system. RateLimit and Overloaded errors trigger soft cooldown. AuthFailure triggers credential disable. This replaces the current inline status-code checks.

## Interface Contract

> **Cross-Role Resolution (C-002)**: Add RequestContext parameter carrying was_compressed (SME) and upstream_headers (TS) to both classify() and to_anthropic_response().

<!-- superseded by C-002 -->
```rust
pub enum ErrorClass {
    RateLimit,
    Overloaded,
    BadRequest,
    AuthFailure,
    NotFound,
    ServerError,
    NetworkError,
    Unknown,
}

pub struct MappedError {
    pub anthropic_status: u16,
    pub error_type: String,
    pub message: String,
    pub retry_after: Option<u32>,
    pub retryable: bool,
    pub error_class: ErrorClass,
}

pub fn classify(upstream_status: u16, body: &[u8]) -> ErrorClass;
pub fn to_anthropic_response(upstream_status: u16, body: &[u8]) -> MappedError;
pub fn is_retryable(class: &ErrorClass) -> bool;
```

## Constraints (RFC 2119)

- MUST map all known upstream error codes (400, 401, 403, 404, 429, 500, 502, 529) to appropriate Anthropic error format
- MUST inject Retry-After header for 429 and 529 responses
- MUST classify errors as retryable or terminal for failover decisions
- MUST NOT modify request handling for 2xx responses
- SHOULD parse upstream error body to extract meaningful error messages when available
- MUST handle malformed upstream error bodies gracefully (fallback to generic message)
- SHOULD integrate with MetricsCollector for error classification counts (see SA-07)
- MUST run after the compression pipeline (SA-06) -- error mapping processes upstream responses, not requests

## Test Approach

- **Unit tests:** Mapping correctness for each known upstream status code. Malformed body handling. Retry-After header generation. ErrorClass classification accuracy.
- **Integration tests:** End-to-end error propagation -- upstream 429 returns correct Anthropic 429 with Retry-After. Streaming error event mapping. Error triggers correct cooldown behavior.
- **Regression:** Verify that existing retry/failover logic is preserved when error_map replaces inline checks.
- **Edge cases:** Empty body, non-JSON body, partial JSON body, extremely large error body.

## TODOs

- Catalog all current inline error handling locations to ensure complete migration
- Determine if dev-source error_map.rs has additional error codes not currently handled
- Evaluate whether error messages should be sanitized (strip internal details) before returning to client
- Decide on error body size limit for parsing (avoid OOM on adversarial responses)
