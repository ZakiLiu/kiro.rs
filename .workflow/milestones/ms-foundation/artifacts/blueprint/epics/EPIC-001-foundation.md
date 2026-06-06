---
document: epic
session_id: BLP-kiro-fusion-2026-06-05
epic_id: EPIC-001
title: P0 Foundation
priority: P0
mvp: true
features: [F-008, F-001, F-002]
constraints: [C-001, C-003]
---

# EPIC-001: P0 Foundation (MVP)

The foundation epic delivers the three infrastructure pillars — CredentialIdentity trait, CrossRequestCache, and MetricsCollector — that all subsequent features depend on. These components are orthogonal to existing anti-detection and compression pipelines and MUST NOT weaken them (C-001).

This is the MVP: completing EPIC-001 alone delivers measurable value (reduced token costs via caching, operational visibility via metrics) while establishing the trait contracts consumed by EPIC-002 and EPIC-003.

## Stories Summary

| ID | Title | Size | Trace | Dependencies |
|----|-------|------|-------|-------------|
| S-001 | CredentialIdentity trait | M | F-008, C-003 | None |
| S-002 | CrossRequestCache with LRU and TTL | L | F-001, C-001 | S-001 |
| S-003 | Cache-converter integration | M | F-001 × F-004 | S-001, S-002 |
| S-004 | MetricsCollector with ring buffer | L | F-002 | S-001 |
| S-005 | Admin API metrics endpoints | M | F-002 | S-004 |

## Story Details

### S-001: Implement CredentialIdentity Trait

**User Story**: As a system architect, I want a unified `CredentialIdentity` trait with domain-separated identity derivation, so that cache keys and anti-detection fingerprints are cryptographically independent and cannot be correlated by upstream.

**Size**: M (3 pts)

**Trace**: F-008, C-003

**Acceptance Criteria**:
1. A `CredentialIdentity` trait MUST be defined in a shared module with `derive_identity(domain: &str) -> String` method
2. Identity derivation MUST use HMAC-based domain separation (e.g., `HMAC(credential_secret, "cache")` vs `HMAC(credential_secret, "detection")`) so that cache identity and detection fingerprint are unlinkable
3. All existing credential consumers (token_manager, cooldown, rate_limiter) MUST be refactored to use the trait
4. Existing anti-detection fingerprint behavior MUST NOT change (regression test required)

**Dependencies**: None — this is the root story.

---

### S-002: Implement CrossRequestCache with LRU Eviction and TTL Tiers

**User Story**: As a power user, I want cross-request prefix caching that maps `(credential_identity, content_fingerprint)` to a `conversation_id`, so that repeated prompts with the same prefix reuse upstream computation and reduce latency and cost.

**Size**: L (5 pts)

**Trace**: F-001, C-001

**Acceptance Criteria**:
1. `CrossRequestCache` MUST implement an LRU cache with configurable max capacity (default: 1000 entries)
2. TTL tiers MUST match the existing balance-cache pattern: high-frequency 10 min, low-frequency 30 min, configurable via `CompressionConfig` extension
3. Cache key MUST use the `CredentialIdentity::derive_identity("cache")` output, NOT raw credential data
4. Cache MUST be thread-safe (`Arc<RwLock<...>>` or lock-free) and integrated into `AppState`

**Dependencies**: S-001 (CredentialIdentity trait)

---

### S-003: Integrate Cache with Converter for forced_conversation_id Injection

**User Story**: As a power user, I want the converter to automatically inject `forced_conversation_id` into Kiro requests when a cache hit exists, so that upstream prefix caching is activated transparently without client-side changes.

**Size**: M (3 pts)

**Trace**: F-001 × F-004 (partial)

**Acceptance Criteria**:
1. `converter::convert_request()` MUST accept an optional `conversation_id` parameter and inject it into the Kiro request payload when present
2. Cache lookup MUST occur in the handler before conversion, using the content fingerprint from the request body
3. Cache insertion MUST occur after a successful response, extracting `conversation_id` from the upstream response (insertion point TBD per OQ-3)
4. When cache is disabled or misses, behavior MUST be identical to current (no `conversation_id` field)

**Dependencies**: S-001, S-002

**Open Question**: OQ-3 — insertion point (stream.rs vs provider.rs) must be resolved before implementation.

---

### S-004: Implement MetricsCollector with Ring Buffer and Request Recording

**User Story**: As a platform operator, I want per-credential request metrics (count, latency, token usage, error rate) stored in a fixed-size ring buffer, so that I can monitor system health without unbounded memory growth.

**Size**: L (5 pts)

**Trace**: F-002

**Acceptance Criteria**:
1. `MetricsCollector` MUST use a ring buffer (fixed capacity, default 10,000 entries) to store `RequestRecord` structs
2. Each `RequestRecord` MUST include: timestamp, credential_id (derived via CredentialIdentity), model, latency_ms, input_tokens, output_tokens, status (success/error), error_type (if applicable)
3. Recording MUST be non-blocking (channel-based or lock-free write) to avoid adding latency to the request path
4. `MetricsCollector` MUST be integrated into `AppState` and invoked from the handler after response completion

**Dependencies**: S-001 (uses CredentialIdentity for credential_id)

---

### S-005: Extend Admin API with Metrics Aggregation Endpoints

**User Story**: As a platform operator, I want Admin API endpoints that aggregate metrics by credential, model, and time window, so that I can query operational dashboards and detect anomalies.

**Size**: M (3 pts)

**Trace**: F-002

**Acceptance Criteria**:
1. `GET /admin/metrics/summary` MUST return aggregated metrics (total requests, avg latency, error rate, token totals) for the last N minutes (query param, default 60)
2. `GET /admin/metrics/by-credential` MUST return per-credential breakdowns
3. `GET /admin/metrics/by-model` MUST return per-model breakdowns
4. All endpoints MUST require `adminApiKey` authentication (existing Admin API auth pattern)

**Dependencies**: S-004 (MetricsCollector must exist)

---

## Epic-Level Acceptance Criteria

1. All 5 stories completed and merged to main
2. `cargo test` passes with no regressions
3. Anti-detection and compression pipelines unaffected (C-001 verified by running existing test suite + manual smoke test)
4. New code covered by unit tests (>80% branch coverage for new modules)
5. Admin UI can display metrics (basic integration, full UI polish deferred)

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Cache key collision across credentials | Low | High | HMAC domain separation (S-001) + unit test with collision detection |
| Ring buffer contention under load | Medium | Medium | Non-blocking channel-based recording (S-004); benchmark under 1000 RPS |
| OQ-3 unresolved blocks S-003 | Medium | High | Spike task to test both insertion points before S-003 starts |
| CredentialIdentity refactoring breaks existing consumers | Low | High | Implement trait with blanket impl for existing types; feature flag rollout |
