---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: NFR-SEC-001
priority: must
category: security
---

# NFR-SEC-001: Anti-Detection Preservation

## User Story

As a **Power User**, I want the new cache and metrics features to preserve the existing anti-detection system's fingerprint diversity and behavioral stealth, so that adding observability does not inadvertently create detectable patterns that compromise my operational security.

## Description

The anti-detection system in kiro.rs is the core competitive moat (SME-04). It comprises three interlocking subsystems: device fingerprint generation (`fingerprint.rs`), credential-user affinity binding (`affinity.rs`), and rate limit simulation (`rate_limiter.rs`). Together, these make each credential appear as a distinct, realistic user with consistent behavioral patterns.

The fusion introduces two new subsystems that interact with credential identity: the cross-request cache (REQ-001) and request metrics (REQ-002). Both require credential-derived keys — and if those keys correlate with anti-detection fingerprints, an upstream observer could link cache access patterns to simulated device identities, effectively de-anonymizing credentials.

This NFR establishes three hard constraints:

1. **Fingerprint diversity MUST be preserved**: No new feature may reduce the fingerprint parameter space or create correlation between credentials that were previously independent.

2. **Cache keys MUST NOT correlate with detection identities**: The CredentialIdentity trait (REQ-008) enforces domain-separated derivation. This NFR validates the separation at the system level — not just at the cryptographic level, but at the behavioral level (access patterns, timing, frequency).

3. **Rate limiting behavior MUST remain realistic**: The rate limiter simulates human-like request timing. Metrics recording and cache operations MUST NOT alter the observable timing characteristics of requests. Specifically, cache hits MUST NOT cause detectably faster response initiation compared to cache misses from the upstream perspective (the upstream sees the same request regardless of cache hit/miss — the cache affects `conversationId` injection, not response speed).

## Acceptance Criteria

1. **Identity Isolation**: Given two credentials A and B, an observer with full access to cache state MUST NOT be able to determine which detection fingerprint belongs to which credential. This is enforced by domain-separated SHA-256 derivation (REQ-008). Tests MUST verify that `cache_identity(A) ↔ detection_identity(A)` correlation is not computable without the seed.

2. **No New Correlation Vectors**: The cross-request cache MUST NOT introduce new observable patterns that correlate with credential identity. Specifically: cache entry timestamps MUST NOT be queryable via the Admin API at per-credential granularity (aggregate only). Cache hit/miss status MUST NOT be exposed in response headers or timing.

3. **CLI Endpoint Preservation**: The existing CLI endpoint (`kiro/endpoint/cli.rs`) MUST continue to function with full anti-detection features. New features (cache, metrics) MUST NOT be exposed via the CLI endpoint — they are Admin API only.

4. **Rate Limiter Independence**: The rate limiter (`rate_limiter.rs`) MUST continue to use `detection_identity()` for rate key derivation. It MUST NOT be modified to use `cache_identity()` or `credential_id()`. Any change to rate limiter key derivation would alter the observed rate limiting pattern.

## Threat Model

| Threat | Vector | Mitigation |
|--------|--------|------------|
| Cache-detection correlation | Observer correlates cache access patterns with fingerprint headers | Domain-separated SHA-256 (REQ-008) — cache key is independent of detection identity |
| Temporal fingerprinting via cache | Observer detects "same user" across requests by analyzing conversation_id reuse timing | TTL expiry limits reuse window; conversation_id scoped per-credential |
| Metrics leaking identity | Admin API exposes per-credential timing data that reveals fingerprint | Aggregate metrics only; no per-credential timing in API responses |
| Rate limiter key change | New features accidentally change rate limiter key derivation | Rate limiter exclusively uses detection_identity(); REQ-008 enforces separation |

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-008 | **Hard dependency** — Domain separation is the primary mitigation |
| REQ-001 | **Constraint target** — Cache must not leak identity |
| REQ-002 | **Constraint target** — Metrics must not leak identity |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SME-04 | Subject Matter Expert | Anti-detection is the core competitive moat |
| SME-01 | Subject Matter Expert | Domain-separated derivation prevents correlation |
| SA-04 | System Architect | CredentialIdentity trait design |
| PM-02 | Product Manager | MUST NOT weaken any existing capability |
| C-001 | Constraint (locked) | MUST NOT weaken anti-detection |
| C-003 | Constraint (locked) | MUST implement domain-separated identities |

## Validation Approach

1. **Unit tests**: Verify `cache_identity()` and `detection_identity()` produce different outputs for the same seed. Verify neither is derivable from the other.
2. **Property tests**: Use `proptest` to verify that for random seeds, `cache_identity` and `detection_identity` outputs have no statistical correlation.
3. **Code review**: Audit all consumers of CredentialIdentity to ensure no cross-domain usage (e.g., rate_limiter accidentally using cache_identity).
4. **Integration test**: Simulate multi-credential traffic and verify that cache entries cannot be attributed to specific detection fingerprints without seed knowledge.
