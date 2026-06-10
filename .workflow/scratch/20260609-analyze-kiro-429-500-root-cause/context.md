# Context: kiro.rs 429/500 Error Root Cause Analysis

**Date**: 2026-06-09
**Scope**: standalone (adhoc debugging)
**Areas**: 429 retry logic, 500 error handling, credential cycling

## Decisions

### Decision 1: Fix 429 Thundering Herd
- **Context**: 429 retry path cycles credentials without backoff delay, amplifying upstream rate limiting ~13x per request
- **Options**:
  1. Add `sleep(retry_delay(attempt))` to 429 path (same as 5xx)
  2. Add fixed delay (e.g., 1s) between 429 retries
  3. Increase cooldown duration from 10s to longer
- **Chosen**: Option 1 — reuse existing exponential backoff
- **Reason**: Consistent with 5xx handling, well-tested, automatic escalation

### Decision 2: Lower Consecutive 429 Bail Threshold
- **Context**: `max_consecutive_429 = available/2 ≈ 13` burns through too many credentials before detecting global rate limit
- **Options**:
  1. Fixed threshold of 3
  2. Dynamic `min(3, available/4)`
  3. Keep current `available/2`
- **Chosen**: Option 1 — fixed threshold of 3
- **Reason**: 3 consecutive different credentials all returning 429 is conclusive evidence of global rate limiting

### Decision 3: Add Total 429 Counter
- **Context**: consecutive_429_count resets on non-429 responses, allowing mixed error scenarios to bypass the bail
- **Chosen**: Add non-resetting total_429_count with threshold 5
- **Reason**: Catches edge cases where 500/network errors intersperse with 429s

## Constraints

### Locked
- Must not change the external API response format
- Must preserve backward compatibility with existing client behavior
- Must not affect non-429 error handling paths
- All changes limited to `src/kiro/provider.rs`

### Free
- Exact backoff timing parameters (base/max) — reuse existing retry_delay()
- Total 429 threshold value — 5 is reasonable, implementer can adjust

### Deferred
- Advanced rate limiting: token bucket or sliding window per upstream endpoint
- Adaptive cooldown: escalating cooldown duration based on 429 frequency
- Request queuing: hold requests during detected global rate limit instead of failing fast

## Code Context

Key code locations:
- `src/kiro/provider.rs:456-710` — MCP call_api retry loop with 429 handling
- `src/kiro/provider.rs:776-1186` — streaming call_api_with_retry retry loop
- `src/kiro/provider.rs:1188-1197` — retry_delay() function (exponential backoff)
- `src/anthropic/error_map.rs:128-139` — AllCredentialsCooling → 429 response mapping
- `src/anthropic/error_map.rs:293-299` — RateLimitTransient → 429 response mapping
