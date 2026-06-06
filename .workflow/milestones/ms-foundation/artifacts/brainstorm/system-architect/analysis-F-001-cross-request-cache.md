# F-001 -- Cross-Request Prefix Cache

> Role: system-architect | Related decisions: SA-02, SA-04, SME-01, PM-01, PM-04

## Architecture

The cross-request cache introduces a new module `src/kiro/prompt_cache.rs` (or `src/kiro/cross_cache.rs` to avoid confusion with the existing `anthropic/cache_tracker.rs`). This module is architecturally distinct from the existing CacheTracker: CacheTracker simulates per-request prefix cache accounting for usage estimation, while CrossRequestCache manages persistent (credential_id, fingerprint) -> conversation_id mappings across requests.

**Module placement:** `src/kiro/cross_cache.rs` owned by the kiro module, since the conversation_id is a Kiro-protocol concept. The cache is injected into AppState as `Arc<CrossRequestCache>` and passed to both `handlers.rs` (pre-request lookup) and `stream.rs` (post-response insertion).

**Data flow:**
1. Pre-request: handler extracts (credential_id, derived_cache_key) from the resolved credential via CredentialIdentity trait (see [F-008](analysis-F-008-shared-identity.md))
2. Lookup returns Option of conversation_id; if Some, inject into the Kiro request payload
3. Post-response: stream parser extracts the conversation_id from upstream response and calls cache.insert()
4. The existing CacheTracker continues to operate independently for usage accounting

**Storage:** In-memory HashMap with LRU eviction. The key is a composite of credential_id (u64) and derived_cache_key ([u8; 32]). The value is a CacheEntry containing conversation_id (String), created_at (Instant), and ttl (Duration).

**Relationship to dev-source:** Dev-source prompt_cache.rs uses a similar (credential, fingerprint) -> conversation_id approach with TTL buckets (5m and 1h). The current project SHOULD adopt the dual-TTL model but MUST implement it independently rather than copying code (see TS-02).

## Interface Contract

```rust
pub struct CrossRequestCache {
    entries: parking_lot::Mutex<LruMap>,
    config: CrossRequestCacheConfig,
}

impl CrossRequestCache {
    pub fn new(config: CrossRequestCacheConfig) -> Self;
    pub fn lookup(&self, credential_id: u64, cache_key: &[u8; 32]) -> Option<String>;
    pub fn insert(&self, credential_id: u64, cache_key: &[u8; 32], conversation_id: String, ttl: Duration);
    pub fn invalidate_credential(&self, credential_id: u64);
    pub fn stats(&self) -> CacheStats;
}

pub struct CacheStats {
    pub total_entries: usize,
    pub hits: u64,
    pub misses: u64,
}
```

The invalidate_credential method MUST be called when a credential enters cooldown or is disabled (hooks into existing report_failure path in provider.rs).

## Constraints (RFC 2119)

- MUST key cache entries on (credential_id, derived_cache_key) to prevent cross-credential leakage
- MUST NOT refresh TTL on cache hit (align with upstream Anthropic prompt cache semantics, consistent with existing CacheTracker behavior)
- MUST enforce max_entries limit via LRU eviction
- SHOULD support dual TTL: 5-minute default and 1-hour extended (configurable)
- MUST invalidate all entries for a credential when that credential is disabled
- MUST NOT block the request path -- cache operations SHOULD complete in under 1ms
- SHOULD expose cache stats via MetricsCollector (see [F-002](analysis-F-002-request-metrics.md))

## Test Approach

- **Unit tests:** Cache insert/lookup/eviction/expiration with mock Instants. Verify LRU ordering. Verify credential invalidation removes correct subset.
- **Integration tests:** End-to-end request flow with cache enabled -- verify conversation_id propagated to upstream request. Verify cache miss on first request, hit on subsequent identical request.
- **Edge cases:** Concurrent insert/lookup from multiple tokio tasks. Cache full (LRU eviction under load). TTL boundary (entry expires between lookup and use).
- **Regression:** Verify existing CacheTracker accounting is unaffected by CrossRequestCache presence.

## TODOs

- Decide on LRU implementation: custom or use `lru` crate (evaluate dependency weight)
- Determine how conversation_id is extracted from upstream Kiro response (event stream parsing point)
- Evaluate whether cache warm-up from previous session is needed (likely not, per cold-start acceptance)
- Profile lock contention under concurrent load (parking_lot::Mutex vs RwLock for read-heavy workload)
