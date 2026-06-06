# Finding: Error Flow Gap -- Scattered Error Handling

> Role: system-architect | Impact: HIGH

## Description

Error handling in the current kiro.rs codebase is distributed across at least four locations with inconsistent translation logic:

1. **handlers.rs** -- HTTP-level error responses for request validation failures and non-streaming upstream errors. Uses ad-hoc status code mapping.
2. **stream.rs** -- Streaming error event handling. Detects error events in the AWS Event Stream and converts them to Anthropic SSE error events. Has its own error message formatting.
3. **provider.rs** -- Retry/failover decisions based on upstream status codes. Contains inline status-code checks (e.g., 429 triggers retry, 402 triggers credential disable) that duplicate classification logic.
4. **cooldown.rs** -- Categorizes failures into FailureLimit, InsufficientBalance, ModelUnavailable, QuotaExceeded. This is a partial error taxonomy that does not cover the full spectrum.

The dev-source project centralizes this in a single `error_map.rs` module with a complete mapping table. The current project lacks this centralization, leading to several problems:

- Inconsistent Retry-After header injection (sometimes present, sometimes missing for the same error type)
- Duplicate classification logic between provider.rs retry decisions and cooldown.rs categorization
- Error messages returned to clients vary in format depending on where the error is caught
- Adding new error types requires changes in multiple files

## Affected Features

- F-003 error-mapping (directly addresses this gap)
- F-002 request-metrics (error classification counts depend on consistent classification)
- All existing error handling code (migration target)

## Recommendation

Implement F-003 error_map module as the single source of truth for error classification and translation. Phase the migration: first add error_map as a parallel path (call it alongside existing logic, compare results in logs), then switch to error_map as the sole path once confidence is established. This phased approach minimizes regression risk while the scattered handling is consolidated.
