# F-003 — Unified Error Mapping

> Role: product-manager | Related decisions: PM-01, PM-04, SA-03

## Architecture

Error handling in the current project is scattered across handlers with inline matching. Dev-source's error_map.rs provides a centralized Kiro-to-Anthropic error translation layer with Retry-After header injection and clear retryable/non-retryable classification. This directly improves client experience: downstream Claude Code/API consumers receive well-formed Anthropic error responses instead of raw Kiro errors or generic 502s.

The error mapping module MUST sit between the provider response path and the handler response construction. It intercepts upstream error responses, maps them to Anthropic error format, and enriches them with actionable metadata (Retry-After, error_type classification).

This is a P1 feature — it depends on stable P0 foundations (cache and metrics) but is critical for production reliability. The current project already classifies some network errors (connection reset, send failure) as transient/502, but lacks structured mapping for application-level errors (auth failures, quota exceeded, model unavailable).

## Interface Contract

- **Input**: Raw Kiro error response (status code, body, headers)
- **Output**: Anthropic-formatted error response with appropriate HTTP status, error type, and optional Retry-After header
- **Error categories**: overloaded (529), rate_limited (429), invalid_request (400), authentication_error (401), not_found (404), api_error (500)
- **Integration with cooldown**: Error classification MUST inform cooldown.rs categorization (FailureLimit, InsufficientBalance, etc.)

## Constraints (RFC 2119)

- MUST map all known Kiro error codes to Anthropic error format
- MUST inject Retry-After header for rate-limited and overloaded responses
- MUST classify errors as retryable vs non-retryable to guide the retry logic in provider.rs
- MUST NOT swallow error details needed for debugging (preserve original error in logs when sensitive-logs is enabled)
- SHOULD provide error-type-specific metrics counters (feeds into F-002)
- SHOULD handle compression-related errors: if input compression still results in upstream 400, the error map MUST identify this as a compression-edge-case for diagnostics

## Test Approach

- Unit tests for each Kiro error code to Anthropic error mapping
- Unit tests for Retry-After header injection logic
- Integration test: simulate upstream 429 response, verify client receives Anthropic-formatted 429 with Retry-After
- Regression test: verify existing network error classification (502 for transient) is preserved

## TODOs

- Catalog all Kiro upstream error codes and their current handling in the project
- Study dev-source error_map.rs for the full mapping table
- Determine if error mapping should be a middleware (Axum layer) or called explicitly from handlers
- Coordinate with SA role on integration point with existing retry logic
