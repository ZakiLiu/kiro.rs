---
document: architecture
session_id: BLP-kiro-fusion-2026-06-05
adr_id: ADR-002
title: Cross-Request Cache Topology
status: proposed
created: 2026-06-05
deciders:
  - fusion-blueprint-generator
traces:
  - brainstorm/decisions.md#cache-topology
---

# ADR-002: Cross-Request Cache Topology

## Context

Two cache mechanisms exist across the codebases:

1. **CacheTracker** (current project, `anthropic/cache_tracker.rs`): Per-request cache breakpoint
   tracking for Anthropic's prompt caching. Estimates cost savings by tracking which content
   blocks are cacheable. Operates within a single request lifecycle.

2. **Cross-request prompt cache** (dev-source): Stores `conversation_id` mappings across requests
   so that subsequent requests from the same user/credential can reuse upstream conversation
   context, reducing input token costs significantly.

The fusion MUST support both mechanisms simultaneously — CacheTracker for per-request cost
estimation, and a new cross-request cache for conversation reuse.

## Decision

We MUST layer a `CrossRequestCache` ON TOP of the existing `CacheTracker`, not replace it.

**Design:**

```rust
pub struct CrossRequestCache {
    entries: parking_lot::Mutex<LruCache<CacheKey, CacheEntry>>,
    config: CacheConfig,
}

pub struct CacheKey {
    cache_identity: IdentityHash,  // From CredentialIdentity::cache_identity()
    model: String,
    content_hash: u64,             // xxHash of system prompt + first user message
}

pub struct CacheEntry {
    conversation_id: String,
    created_at: Instant,
    last_accessed: Instant,
    ttl: Duration,
    state: CacheState,
}
```

**Key properties:**

- **LRU eviction**: `lru` crate with `parking_lot::Mutex` for thread safety. Maximum 1000 entries (configurable).
- **Two TTL tiers**: Short-tier (5 minutes) for one-shot requests; long-tier (1 hour) for
  multi-turn conversations detected by repeated cache hits on the same key.
- **Credential scoping**: Cache keys include `cache_identity` from ADR-001, ensuring
  conversation IDs are never shared across credentials.
- **Content hashing**: xxHash (non-cryptographic, fast) of system prompt + first user message
  to identify semantically equivalent requests.
- **Lifecycle**: See state machine in `_index.md` Section 6.

**Integration point**: `anthropic/converter.rs` checks `CrossRequestCache` before building
the Kiro request. On cache hit, the `conversation_id` is injected into the outgoing request.
On upstream response, the `conversation_id` from the response is stored back.

## Alternatives Considered

### (a) Replace CacheTracker

Remove `CacheTracker` entirely and use `CrossRequestCache` for all caching concerns.

**Rejected**: CacheTracker serves a fundamentally different purpose (per-request cost estimation
vs cross-request conversation reuse). Replacing it would lose cost estimation functionality.

### (b) External Redis Cache

Use Redis for cross-request storage with TTL support.

**Rejected**: Adds an external dependency to what is currently a zero-dependency single-binary
deployment. The operational complexity is not justified for the expected cache size (~1000 entries).

### (c) File-Based Persistence

Persist cache to disk for survival across restarts.

**Rejected**: Conversation IDs have short upstream validity (minutes to hours). Cache warm-up
after restart is fast enough that persistence adds complexity without meaningful benefit.

## Consequences

**Positive:**
- Two independent cache layers, each optimized for its purpose.
- CacheTracker continues to provide per-request cost estimation unchanged.
- CrossRequestCache reduces input token costs for repeated/multi-turn conversations.
- Zero external dependencies; bounded memory via LRU eviction.

**Negative:**
- Two cache systems to understand and maintain.
- Cache key computation adds ~1us per request (xxHash + SHA-256 lookup).
- Credential rotation invalidates all cache entries for that credential.

**Risks:**
- Stale conversation IDs MAY cause upstream 400 errors if the upstream session has expired.
  Mitigation: On 400 with a cached conversation_id, invalidate the entry and retry without it.

## Implementation Notes

- `CrossRequestCache` MUST be wrapped in `Arc` and stored in `AppState` for handler access.
- TTL tier promotion (short -> long) happens on second cache hit within the short TTL window.
- Admin API SHOULD expose cache stats: hit rate, entry count, eviction count.
- Cache invalidation on credential cooldown MUST use `cache_identity` from ADR-001.
