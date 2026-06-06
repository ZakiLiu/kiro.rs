# F-001 — Cross-Request Prefix Cache

> Role: subject-matter-expert | Related decisions: SME-01, SA-02, SA-04

## Architecture

The cross-request cache bridges the gap between the existing per-request `CacheTracker` (SHA-256 prefix fingerprinting, `cache_tracker.rs`) and the dev-source's `prompt_cache.rs` (LRU + TTL + conversation_id mapping). The target architecture introduces a `CrossRequestCache` module that maps `(credential_id, prefix_fingerprint) -> conversation_id`, enabling session reuse across separate HTTP requests.

Key modules:
- `anthropic/cross_request_cache.rs` — LRU map with TTL-bucketed eviction (5m default, 1h for explicit `cache_control.ttl = "1h"`)
- Integration point in `anthropic/handlers.rs` — after `CacheTracker::build_profile()`, look up existing conversation_id before constructing the Kiro request
- The `CredentialIdentity` trait (F-008) provides the fingerprint seed; the cache module consumes it without owning fingerprint generation

The dev-source uses a flat `HashMap<(String, String), CacheEntry>` keyed by `(credential_hash, content_hash)`. The current project's `CacheTracker` already computes `prefix_fingerprint` as `[u8; 32]` via rolling SHA-256 — this MUST be reused as the content hash component. The credential component MUST come from `CredentialIdentity::cache_identity()`, which MAY differ from the anti-detection identity to avoid correlation (see SA-04).

## Interface Contract

```rust
pub struct CrossRequestCache {
    entries: Mutex<LruCache<CacheKey, ConversationEntry>>,
    max_entries: usize,
}

#[derive(Hash, Eq, PartialEq)]
struct CacheKey {
    credential_identity: [u8; 32],
    prefix_fingerprint: [u8; 32],
}

struct ConversationEntry {
    conversation_id: String,
    created_at: Instant,
    ttl: Duration,
    hit_count: u64,
}

impl CrossRequestCache {
    pub fn lookup(&self, key: &CacheKey) -> Option<String>;
    pub fn insert(&self, key: CacheKey, conversation_id: String, ttl: Duration);
    pub fn evict_by_credential(&self, credential_id: [u8; 32]);
}
```

Consumers: `anthropic/handlers.rs` (request path), `admin/handlers.rs` (stats endpoint), `kiro/provider.rs` (conversation_id injection).

## Constraints (RFC 2119)

- The cache MUST use the existing `CacheTracker::build_profile()` fingerprint as content hash — reimplementing prefix hashing is prohibited.
- The cache MUST support TTL buckets: 5-minute default and 1-hour for explicit `cache_control.ttl = "1h"`.
- The cache MUST use LRU eviction with a configurable `max_entries` (default 10,000) to bound memory.
- The cache MUST NOT share the raw fingerprint identity between anti-detection (`affinity.rs`) and cache lookup — `CredentialIdentity` MUST provide a derived identity for cache use (see SME-01, SA-04).
- The cache SHOULD track hit/miss counts for integration with F-002 (request-metrics).
- The cache MAY support warm-up from persisted state on restart, but this is not required for P0.

## Test Approach

- Unit tests: LRU eviction correctness, TTL expiry, fingerprint collision resistance (use known SHA-256 vectors).
- Integration tests: Multi-request conversation reuse — send two requests with identical prefix, verify same `conversation_id` is injected.
- Property tests: Fuzzing `CacheKey` generation with random payloads to verify no false positives across 10,000 iterations.
- Regression: Ensure `CacheTracker` existing tests still pass unchanged — the cross-request cache is additive, not a replacement.

## TODOs

- Measure memory footprint of 10,000 `ConversationEntry` instances to validate default `max_entries`.
- Study dev-source `prompt_cache.rs` LRU implementation for edge cases around concurrent access patterns.
- Determine whether `conversation_id` values from upstream have a fixed format or are opaque strings.
- Evaluate whether `parking_lot::Mutex` or `tokio::sync::Mutex` is more appropriate given the async request path.
