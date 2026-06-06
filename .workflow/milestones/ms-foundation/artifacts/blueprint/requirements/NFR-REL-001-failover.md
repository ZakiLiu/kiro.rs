---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: NFR-REL-001
priority: must
category: reliability
---

# NFR-REL-001: Failover Continuity and Error-Driven Retry

## User Story

As a **Power User**, I want the existing credential failover mechanism to continue working exactly as it does today, with the new error classification driving retry decisions and cache invalidation happening automatically when credentials enter cooldown, so that service reliability is maintained or improved — never degraded — by the fusion.

## Description

The current kiro.rs credential failover system is battle-tested: credentials are sorted by priority, requests fail over to the next available credential on transient errors, and the cooldown system (`cooldown.rs`) manages recovery timing with category-specific durations (FailureLimit, InsufficientBalance, ModelUnavailable, QuotaExceeded). This system MUST remain unchanged in behavior.

The fusion adds two new interactions with the failover system:

1. **Error classification drives retry** (REQ-003): The new ErrorMapper classifies upstream errors into retryable vs non-retryable categories. This classification MUST align with the existing retry logic in `provider.rs`. Retryable errors (RateLimit, Overloaded, ServerError, NetworkError) trigger failover. Non-retryable errors (BadRequest, AuthFailure, NotFound) return immediately. The ErrorMapper does not change retry behavior — it provides a cleaner classification that the existing retry logic consumes.

2. **Cache invalidation on cooldown** (REQ-001): When a credential enters cooldown (via `report_failure()`), all cross-request cache entries for that credential MUST be bulk-invalidated. This prevents stale conversation_id reuse on a credential that is temporarily or permanently unavailable. The invalidation hook is added to the existing cooldown entry point — it does not create a new code path.

This NFR ensures that neither interaction weakens the existing failover guarantees. The retry budget (2 retries per credential, 3 retries per request) MUST NOT change. The cooldown category durations MUST NOT change. The credential selection priority order MUST NOT change.

## Acceptance Criteria

1. **Failover Preserved**: The existing retry logic (single credential: max 2 retries; single request: max 3 retries across credentials) MUST NOT be modified. Error classification from ErrorMapper MUST map to the same retry/no-retry decisions that the current inline logic produces. A comprehensive test MUST verify that every existing error scenario produces identical retry behavior before and after ErrorMapper integration.

2. **Cache Invalidation on Cooldown**: When `report_failure()` transitions a credential to cooldown state, the system MUST bulk-remove all CrossRequestCache entries where `credential_id` matches the cooling credential. The invalidation MUST complete within the existing `report_failure()` call — no deferred or async cleanup.

3. **Error-Retry Alignment**: The ErrorMapper's `ErrorClass::is_retryable()` method MUST produce identical retry decisions to the current inline logic for all known upstream status codes. A mapping verification test MUST compare ErrorMapper output against a hardcoded table of current behavior.

4. **Cooldown Category Preservation**: The four cooldown categories (FailureLimit, InsufficientBalance, ModelUnavailable, QuotaExceeded) and their associated durations MUST NOT be modified. The ErrorMapper classifies errors; the cooldown system manages recovery timing. These are separate concerns.

## Interaction Diagram

```
Request arrives
  → Provider selects credential (priority order, existing logic)
  → Upstream call
  → On error:
      ErrorMapper.classify(status, body) → ErrorClass
      if ErrorClass.is_retryable():
        report_failure(credential) → cooldown
          → CrossRequestCache.invalidate_credential(credential_id)  [NEW]
        retry with next credential (existing logic)
      else:
        ErrorMapper.to_anthropic_response() → return to client
```

## Retry Budget (Unchanged)

| Scope | Max Retries | Behavior |
|-------|------------|----------|
| Per credential | 2 | Same credential retried up to 2 times |
| Per request | 3 | Total retries across all credentials |
| Global circuit breaker | MODEL_TEMPORARILY_UNAVAILABLE | All credentials disabled |

## Cache Invalidation Triggers

| Cooldown Category | Cache Action |
|-------------------|-------------|
| FailureLimit | Invalidate all entries for credential_id |
| InsufficientBalance | Invalidate all entries for credential_id |
| ModelUnavailable | Invalidate all entries for credential_id |
| QuotaExceeded | Invalidate all entries for credential_id |
| Credential re-enabled | No action (cache rebuilds organically on cache misses) |

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-003 | **Integration** — ErrorMapper classification feeds retry decisions |
| REQ-001 | **Integration** — Cache invalidation on cooldown entry |
| REQ-008 | **Indirect** — credential_id() used for bulk invalidation key |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-05 | System Architect | Credential management split preserved |
| SA-06 | System Architect | Compression pipeline preserved; error mapping runs after |
| SA-03 | System Architect | ErrorMapper classification drives retry |
| PM-02 | Product Manager | MUST NOT weaken existing capabilities |
| SME-04 | Subject Matter Expert | Anti-detection mechanisms preserved through failover |
| TS-01 | Test Strategist | Critical paths MUST have test coverage |
| C-002 | Constraint (locked) | Preserve credential management split |

## Regression Test Strategy

1. **Before/after comparison**: Run existing failover tests before ErrorMapper integration. Record retry counts, credential selection order, and cooldown transitions. After integration, verify identical behavior.
2. **ErrorClass mapping table test**: Hardcode a table of (upstream_status, expected_retry_decision) pairs representing current behavior. Assert ErrorMapper produces identical decisions for every entry.
3. **Cache invalidation test**: Simulate credential cooldown and verify all cache entries for that credential are removed. Verify entries for other credentials are untouched.
4. **End-to-end failover test**: Simulate multi-credential scenario with staggered failures. Verify request succeeds by failing over to healthy credential. Verify cache entries for failed credentials are invalidated.
