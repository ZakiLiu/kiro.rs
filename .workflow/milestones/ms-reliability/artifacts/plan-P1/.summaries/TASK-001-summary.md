# TASK-001: Create ErrorMapper module with classify(), to_anthropic_response(), and ErrorRequestContext

## Changes
- `src/anthropic/error_map.rs`: Created new module with ErrorCategory enum (12 variants), classify() function, to_anthropic_response() function, ErrorRequestContext struct, and 23 unit tests
- `src/anthropic/mod.rs`: Added `pub(crate) mod error_map;` registration

## Verification
- [x] File src/anthropic/error_map.rs exists with `pub enum ErrorCategory` (12 variants: InputTooLong, ImproperlyFormedRequest, CompressionInduced400, QuotaExhausted, NoCredentials, AllCredentialsCooling, RateLimitTransient, NetworkTransient, ModelUnavailable, AuthFailure, ServerTransient, Unknown): verified by compilation
- [x] classify() function with signature `pub fn classify(err: &anyhow::Error, ctx: &ErrorRequestContext) -> ErrorCategory`: verified by 15 test cases
- [x] to_anthropic_response() returning `axum::response::Response`: verified by 8 test cases
- [x] ErrorRequestContext struct with was_compressed, request_body_bytes, compression_iterations fields + derive(Default): verified by test_error_request_context_default
- [x] Retry-After header for AllCredentialsCooling with clamp [60, 300]: verified by test_response_retry_after_clamping_low (10→60) and test_response_retry_after_clamping_high (999→300)
- [x] CompressionInduced400 detected when was_compressed==true AND error contains "Improperly formed request": verified by test_classify_compression_induced_400
- [x] mod.rs contains `pub(crate) mod error_map`: verified by compilation
- [x] `cargo test error_map` passes: 23 passed, 0 failed
- [x] No clippy warnings in error_map.rs: verified (existing warnings in converter.rs/handlers.rs are pre-existing, not introduced by this task)

## Tests
- [x] `cargo test error_map`: 23 passed, 0 failed
- [x] `cargo clippy -- -D warnings`: no warnings from error_map.rs (5 pre-existing warnings in other files)

## Deviations
- Added `#![allow(dead_code)]` at module level since this module is not yet consumed by handlers.rs (will be wired in TASK-002). This is expected and standard practice for newly created modules.
- Used `#[derive(Default)]` instead of manual `impl Default` per clippy `derivable_impls` lint recommendation.

## Notes
- The `Unknown` variant maps to 500 (not 502 like the current fallback in handlers.rs). This is intentional — truly unrecognizable errors should be 500 Internal Server Error, while `ServerTransient` covers the 502 case.
- `AuthFailure` and `ModelUnavailable` variants are present in the enum and classify() but are not currently triggered by handlers.rs error strings — they serve as future classification points when provider.rs surfaces these errors to the handler layer.
- Pre-existing clippy warnings in converter.rs (collapsible_if) and handlers.rs (doc_lazy_continuation, collapsible_match, unnecessary_cast) are outside task scope.
