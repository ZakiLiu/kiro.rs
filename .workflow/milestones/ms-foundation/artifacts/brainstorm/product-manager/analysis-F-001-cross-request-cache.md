# F-001 — Cross-Request Prefix Cache

> Role: product-manager | Related decisions: PM-01, PM-04, SA-02, SA-04, SME-01

## Architecture

The cross-request prefix cache represents the highest-impact cost optimization feature identified from dev-source. The current project's cache_tracker.rs provides only per-request SHA-256 simulation with no cross-request persistence. Dev-source's prompt_cache.rs demonstrates a proven approach: mapping (credential, fingerprint) to conversation_id via LRU eviction with TTL buckets (5m for active, 1h for idle).

The product architecture MUST integrate with existing fingerprint infrastructure through the CredentialIdentity shared trait (see [F-008](analysis-F-008-shared-identity.md)). This ensures cache key generation reuses the established anti-detection fingerprint pipeline without creating a parallel identity system.

Target module placement: `src/kiro/prompt_cache.rs` as a new module alongside the existing cache_tracker.rs, which continues to handle per-request simulation while prompt_cache handles cross-request conversation_id reuse.

## Interface Contract

- **Input**: (CredentialIdentity fingerprint, message prefix hash) tuple from request pipeline
- **Output**: Optional conversation_id for cache hit, new conversation_id registration on miss
- **Metrics integration**: Cache hit/miss ratio MUST be exposed via the metrics system (see [F-002](analysis-F-002-request-metrics.md))
- **Admin API**: Cache stats (hit rate, entry count, eviction count) MUST be queryable through Admin endpoints

## Constraints (RFC 2119)

- MUST use LRU eviction to bound memory consumption (max_entries configurable, default 1000)
- MUST implement TTL tiers: 5-minute for active conversations, 1-hour for idle
- MUST NOT share raw fingerprint identifiers between cache keys and anti-detection affinity bindings; the CredentialIdentity trait MUST allow derivation of independent identifiers per consumer (see SA-04)
- SHOULD support configurable cache size via config.json without restart
- MAY implement cache warming on startup from persisted state, but this is P2 scope

## Test Approach

- Unit tests for LRU eviction correctness (insert, hit, miss, evict ordering)
- Unit tests for TTL expiration (active vs idle buckets)
- Integration test: sequential requests with same prefix MUST produce cache hit on second request
- Property test: cache size MUST NOT exceed max_entries under random workload

## TODOs

- Study dev-source prompt_cache.rs implementation for LRU data structure choice (std HashMap + VecDeque vs lru crate)
- Benchmark memory overhead per cache entry to calibrate default max_entries
- Validate that fingerprint-derived cache keys do not leak cross-credential correlation to upstream
- Define Admin UI widget for cache stats visualization (coordinate with UI role)
