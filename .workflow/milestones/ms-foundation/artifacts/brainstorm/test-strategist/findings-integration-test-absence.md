# Finding: No Integration Tests Exist

> Role: test-strategist | Impact: MEDIUM

## Description

All 378 existing tests are unit tests within `#[cfg(test)]` modules. The project has no `tests/` directory for integration tests. Cross-module interactions are untested:

- converter produces a KiroRequest, but no test verifies provider can send it.
- stream parses events, but no test verifies the full chain from HTTP response bytes to Anthropic SSE output.
- compressor reduces request size, but no test verifies the compressed request succeeds upstream.
- cache_tracker simulates per-request cache, but no test verifies it interacts correctly with the converter's cache breakpoint insertion.

The new features amplify this gap:
- F-001 (cache) depends on F-008 (CredentialIdentity) for key derivation — this cross-module contract is only testable at the integration level.
- F-003 (error mapping) interacts with F-001 compression (§8 Cross-Role Integration) — post-compression errors need integrated testing.

## Affected Features

- F-001 cross-request-cache — Cache key derivation depends on F-008 trait.
- F-003 error-mapping — Post-compression error identification requires integrated test.
- F-008 shared-identity — Trait consumers (cache, affinity) need cross-module verification.
- F-002 request-metrics — Metrics MUST integrate with all features per §8.

## Recommendation

Create a `tests/` directory with focused integration tests for the highest-risk cross-module flows:

1. `tests/cache_identity_integration.rs` — CredentialIdentity trait produces keys that work with cache lookup/insert.
2. `tests/compression_error_integration.rs` — Compressed request triggering upstream error is correctly mapped.
3. `tests/converter_stream_roundtrip.rs` — Anthropic request converted to Kiro, mock response parsed back to Anthropic SSE.

Start with 3 integration test files covering the most critical cross-module boundaries. Each file SHOULD contain 3-5 tests. This provides disproportionate coverage for the investment.
