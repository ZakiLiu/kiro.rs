# Context: Phase 01 — Error & Converter Hardening (Milestone 2)

**Date**: 2026-06-05
**Scope**: micro (Phase 1 of ms-reliability)
**Areas discussed**: ErrorMapper design, integration points, tool name shortening status, RequestContext, CooldownReason mapping

## Decisions

### Decision 1: S-008 (Tool Name Shortening) Already Implemented — SKIP
- **Context**: Codebase exploration revealed tool name shortening is fully implemented
- **Evidence**:
  - converter.rs:340 — `TOOL_NAME_MAX_LEN = 63`
  - converter.rs:343-364 — `shorten_tool_name()` + `map_tool_name()` with SHA-256 suffix
  - stream.rs:1177-1193 — Response-side name restoration via `tool_name_map`
  - handlers.rs:1166 — `tool_name_map` propagated from converter to stream
- **Chosen**: Skip S-008 entirely — **Reason**: Feature already exists and works. Zero additional work needed.
- **Impact**: EPIC-002 reduces to 3 stories (S-006 + S-007 + S-009), ~9 story points instead of 12

### Decision 2: ErrorMapper Module Placement
- **Chosen**: `src/anthropic/error_map.rs` — Anthropic module since output format is Anthropic-specific
- **Reason**: Consistent with brainstorm SA analysis; error mapper produces Anthropic-formatted responses

### Decision 3: ErrorMapper Signature (from brainstorm C-002)
- **Chosen**: Dual-function: `classify(status, body, context) -> ErrorClass` + `to_anthropic_response(class, context) -> (StatusCode, Body)`
- **RequestContext struct**: `{ was_compressed: bool, compression_layers: Vec<String>, upstream_headers: Option<HeaderMap> }`
- **Reason**: Separates classification (drives retry/cooldown) from response formatting (drives client experience)

### Decision 4: Integration Strategy (from brainstorm C-003)
- **Consumers**: handlers.rs + stream.rs + provider.rs
- **Approach**: Phase the integration:
  1. First: Create ErrorMapper module with classify() + to_anthropic_response()
  2. Then: Replace inline error handling in handlers.rs (8 predicates → ErrorMapper)
  3. Then: Wire provider.rs to use classify() for retry/cooldown decisions
  4. Finally: Wire stream.rs error events through classify()

### Decision 5: CooldownReason Mapping
- **Evidence**: 7 CooldownReason variants exist, but only 2 are actively triggered (RateLimitExceeded, ModelUnavailable)
- **Chosen**: ErrorMapper ErrorClass variants map 1:1:
  - RateLimit → CooldownReason::RateLimitExceeded
  - ModelUnavailable → CooldownReason::ModelUnavailable
  - AuthFailure → CooldownReason::AuthenticationFailed
  - ServerError → CooldownReason::ServerError
  - QuotaExhausted → disable credential (not cooldown)
  - InvalidRequest → no cooldown (bail immediately)
  - NetworkTransient → no cooldown (retry without credential change, per Round 11 decision)

## Constraints

### Locked

1. **ErrorMapper dual-function signature**: classify() + to_anthropic_response() with RequestContext struct. (Source: brainstorm C-002)

2. **ErrorMapper consumers**: handlers.rs + stream.rs + provider.rs (union). (Source: brainstorm C-003)

3. **S-008 SKIP**: Tool name shortening already implemented. No work needed. (Source: codebase exploration)

4. **CooldownReason 1:1 mapping**: ErrorClass categories must map to existing CooldownReason variants. Zero behavioral change in cooldown/retry logic. (Source: EPIC-002 AC)

5. **Existing error predicates preservation**: The 8 predicate functions in handlers.rs (is_input_too_long_error, is_quota_exhausted_error, etc.) can be replaced by ErrorMapper, but behavior must be identical. (Source: EPIC-002 AC)

6. **Retry-After header handling**: 429/503 responses MUST include Retry-After. Current logic clamps to [60s, 300s]. ErrorMapper MUST preserve this clamping. (Source: coding-conventions spec)

7. **Network error treatment**: Network errors (connection reset, DNS failure) MUST NOT trigger credential cooldown (Round 11 decision). ErrorMapper must classify these as NetworkTransient, not ServerError. (Source: coding-conventions spec)

8. **sensitive-logs gating**: Diagnostic details (request body, compression context) MUST only be logged when sensitive-logs feature is enabled. (Source: CLAUDE.md constraint)

### Free

9. **ErrorClass enum design**: Implementer MAY choose between 6 categories (from brainstorm) or the full 7 from exploration (adding NetworkTransient). Research suggests: 7 categories matches the existing inline logic more precisely.

10. **RequestContext allocation**: Implementer MAY thread RequestContext through function params or use request extensions. Research suggests: function params are simpler and match existing patterns (no Axum extensions used for this purpose currently).

11. **Error message format**: Implementer MAY choose exact error message text in to_anthropic_response(), as long as the format matches Anthropic API spec (type + error.type + error.message). Research suggests: reuse existing message text from handlers.rs predicates for backward compatibility.

### Deferred

12. **Token refresh error handling unification**: token_manager.rs has its own inline error handling (lines 307-490) that could also use ErrorMapper. Deferred to avoid scope creep — token refresh errors are internal, not client-facing.

13. **Admin API error unification**: admin/error.rs has its own error type. Deferred — admin errors are a separate concern from proxy errors.

## Code Context

### ErrorMapper Integration Points (60+ locations)

| Area | File | Key Lines | Current Handling | ErrorMapper Action |
|------|------|-----------|-----------------|-------------------|
| 400 Bad Request | provider.rs | 561-611, 900-952 | Inline with is_input_too_long() | classify → InvalidRequest |
| 401/403 Auth | provider.rs | 615-637, 956-1002 | report_failure + cooldown | classify → AuthFailure |
| 402 Quota | provider.rs | 550, 868-896 | report_quota_exhausted | classify → QuotaExhausted |
| 429 Rate Limit | provider.rs | 639-667, 1005-1043 | set_cooldown + Retry-After | classify → RateLimit |
| 408/5xx Server | provider.rs | 671-697, 1048-1079 | retry with backoff | classify → ServerError |
| Network errors | provider.rs | 825-841 | no credential push | classify → NetworkTransient |
| Error predicates | handlers.rs | 142-205 | 8 inline functions | Replace with ErrorMapper |
| Error formatting | handlers.rs | 454-568 | map_kiro_provider_error_to_response | Replace with to_anthropic_response |
| Stream errors | stream.rs | 678-714 | Event::Error handling | classify for retry decision |

### Existing Patterns to Follow

- Provider retry loop at provider.rs:721 — ErrorMapper.classify() replaces inline status matches
- handlers.rs map_kiro_provider_error_to_response() at line 454 — entire function replaced by ErrorMapper
- CooldownReason assignments — ErrorMapper provides the category, cooldown.rs consumes it
