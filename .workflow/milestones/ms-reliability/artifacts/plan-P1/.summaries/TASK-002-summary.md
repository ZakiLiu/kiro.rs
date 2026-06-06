# TASK-002: Integrate ErrorMapper into handlers.rs, stream.rs, and provider.rs

## Changes
- `src/anthropic/handlers.rs`: Removed 8 inline error predicate functions (`is_input_too_long_error`, `is_quota_exhausted_error`, `is_no_credentials_error`, `is_all_credentials_cooling_down_error`, `is_transient_upstream_error`, `is_improperly_formed_request_error`, `is_network_error`) and `NETWORK_ERROR_PATTERNS` constant (~130 lines deleted). Rewrote `map_kiro_provider_error_to_response` to construct `ErrorRequestContext` from `AdaptiveCompressionOutcome` and delegate to `error_map::classify()` + `error_map::to_anthropic_response()`. Added `adaptive_outcome` field to `StreamRequestContext` and `NonStreamRequestContext`. Removed 5 handler-level tests for deleted predicates (covered by error_map tests). Updated test for `test_improperly_formed_request_message_mentions_common_causes` to pass new `None` argument.
- `src/anthropic/stream.rs`: Added import for `error_map::{self, ErrorCategory, ErrorRequestContext}`. Enhanced `Event::Error` handler with structured error classification logging via `error_map::classify()`.
- `src/anthropic/error_map.rs`: Removed module-level `#![allow(dead_code)]`, replaced with targeted `#[allow(dead_code)]` on `AuthFailure`/`ServerTransient` variants and `#[cfg_attr(...)]` on `compression_iterations` field.
- `src/kiro/provider.rs`: UNCHANGED — verified all `anyhow::bail!` strings remain compatible with `classify()` patterns.

## Verification
- [x] handlers.rs no longer contains the 8 predicate functions or NETWORK_ERROR_PATTERNS: verified via grep, zero matches
- [x] handlers.rs imports and uses error_map::classify + to_anthropic_response: `use super::error_map::{self, ErrorRequestContext};` at line 38, calls at lines 403-404
- [x] ErrorRequestContext populated from AdaptiveCompressionOutcome: `was_compressed: adaptive_outcome.is_some()`, `compression_iterations: adaptive_outcome.map(|o| o.iters)`
- [x] stream.rs imports error_map types: `use crate::anthropic::error_map::{self, ErrorCategory, ErrorRequestContext};`
- [x] provider.rs UNCHANGED: not in `git diff --name-only` output, CooldownReason::RateLimitExceeded call sites unchanged
- [x] provider.rs bail! strings compatible: verified 'Input is too long', '所有凭据已用尽', '没有可用的凭据', '所有凭据均处于冷却/速率限制', 'Improperly formed request' all present
- [x] CompressionInduced400 response includes diagnostic hint: "This may have been caused by input compression altering the message structure."

## Tests
- [x] `cargo test`: 433 passed; 0 failed; 0 ignored
- [x] `cargo clippy -- -D warnings`: no NEW warnings (5 pre-existing warnings in converter.rs and unmodified handlers.rs code)

## Deviations
- None. All convergence criteria met.

## Notes
- Net result: -181 lines (51 added, 232 removed) — significant simplification of handlers.rs
- The old `map_kiro_provider_error_to_response` had a behavioral difference for `AllCredentialsCooling` — it did NOT clamp `retry_after` to [60, 300], while `error_map::to_anthropic_response` does. This is intentional: the error_map version adds proper clamping as documented in the TASK-001 design. Since the provider already clamps `retry_after_secs` within its own logic, this change has no practical impact.
- Pre-existing clippy warnings (5) are not caused by this task and exist on master: 2x collapsible_if in converter.rs, 1x doc_lazy_continuation in handlers.rs, 1x collapsible_match in handlers.rs, 1x unnecessary_cast in handlers.rs
