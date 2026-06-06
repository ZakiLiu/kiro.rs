# F-003 — Unified Error Mapping

> Role: test-strategist | Related decisions: TS-01, TS-05

## Architecture

The error mapping module test suite MUST exhaustively cover the translation from upstream Kiro error responses to Anthropic-compatible error responses. Dev-source implements a dedicated `error_map.rs` module (see design-research). The current project handles errors inline in handlers.rs, which has 17 existing tests that partially cover error paths.

Key testable components:
- **Status code mapping** — Each upstream status code (400, 401, 402, 403, 429, 500, 502, 503) maps to a specific Anthropic error type and message.
- **Retry-After header injection** — 429 and 503 responses MUST include Retry-After with appropriate backoff values.
- **Retryable vs non-retryable classification** — Drives the provider's retry logic.
- **Error body parsing** — Upstream error bodies may contain structured JSON with error details.
- **Interaction with compression** — Errors triggered after compression (see SA-06, §8 Cross-Role Integration) MUST be correctly identified.

## Interface Contract

> **Cross-Role Resolution (C-002)**: Replace headers param with RequestContext struct; align with SA's two-function pattern (classify + to_anthropic_response).

<!-- superseded by C-002 -->
- `ErrorMap::translate(status: StatusCode, body: &[u8], headers: &HeaderMap) -> AnthropicError` — Pure function, no side effects.
- `AnthropicError` — MUST contain: type (error type string), message (human-readable), status_code (HTTP status), retry_after (Option<Duration>).
- `ErrorMap::is_retryable(status: StatusCode) -> bool` — Classification for provider retry logic.

## Constraints (RFC 2119)

- Every upstream status code MUST have an explicit mapping — no fallthrough to generic "internal error."
- 429 responses MUST include Retry-After header in the translated response.
- 502 responses from network errors (connection reset, timeout) MUST be classified as retryable.
- 402 (insufficient balance) MUST NOT be classified as retryable.
- Error messages MUST NOT leak upstream Kiro-specific details to the Anthropic client.
- The mapping function MUST be pure (no I/O, no state) to ensure deterministic testing.

## Test Approach

**Unit tests (≥ 15 tests):**
1. Map 400 — bad request passthrough with sanitized message.
2. Map 401 — authentication error.
3. Map 402 — insufficient balance, non-retryable.
4. Map 403 — permission denied.
5. Map 429 — rate limited with Retry-After extraction from upstream header.
6. Map 429 — rate limited with default Retry-After when upstream omits header.
7. Map 500 — internal server error, retryable.
8. Map 502 — bad gateway, retryable.
9. Map 503 — service unavailable with Retry-After.
10. Unknown status code (e.g., 418) — maps to generic error without panic.
11. Empty body — graceful handling.
12. Malformed JSON body — graceful handling.
13. is_retryable classification — verify all retryable codes.
14. is_retryable classification — verify all non-retryable codes.
15. Error message sanitization — verify Kiro-specific terms are stripped.

**Property-based tests:**
- For any valid HTTP status code (100-599), `translate` MUST NOT panic.
- For any byte sequence as body, `translate` MUST NOT panic.

**Integration test:**
- Error mapping after compression pipeline failure (§8): compress request, simulate upstream 400 "Improperly formed request", verify error_map produces correct Anthropic error.

## TODOs

- Study upstream Kiro error response body formats (JSON structure, field names).
- Determine Retry-After backoff strategy: fixed, exponential, or upstream-echoed.
- Coordinate with cooldown.rs — error_map classification SHOULD align with CooldownReason enum.
