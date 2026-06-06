---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-001
priority: must
wave: P0
---

# REQ-001: Cross-Request Prefix Cache

## User Story

As a **Power User**, I want the proxy to automatically reuse conversation_id across requests with identical prefixes, so that I reduce upstream token costs by leveraging Kiro's server-side prefix cache without manual session management.

## Description

The current kiro.rs has a per-request `CacheTracker` (in `cache_tracker.rs`) that simulates prefix cache economics for cost reporting. However, it does not persist conversation_id across requests — every request starts a fresh conversation upstream, forfeiting prefix cache reuse.

The cross-request cache introduces an LRU-based in-memory mapping from `(credential_id, fingerprint_hash)` to `conversation_id`. When a request arrives, the handler looks up a matching cache entry and injects the cached `conversation_id` into the Kiro request via the converter (see REQ-004). When a successful response returns a conversation_id, `stream.rs` inserts it into the cache. This layering is orthogonal to the existing `CacheTracker` — it MUST NOT modify or replace the per-request cache simulation.

Cache keys are derived via the `CredentialIdentity` trait (see REQ-008) using domain-separated SHA-256 hashing. This ensures cache keys are cryptographically independent from anti-detection fingerprints, preventing upstream correlation attacks. TTL is tiered: short-lived (5 minutes default) for exploratory conversations, extended (1 hour) for sustained sessions where prefix reuse is most valuable. TTL classification is based on conversation turn count heuristic — single-turn requests get short TTL, multi-turn get extended.

## Acceptance Criteria

1. **Cache Hit**: When a request matches an existing cache entry (same credential_id + cache_identity), the system MUST inject the cached `conversation_id` into the outgoing Kiro request. The cache hit MUST be recorded in metrics (see REQ-002).

2. **Cache Miss + Insert**: When no cache entry exists and the upstream response contains a `conversation_id`, the system MUST insert a new entry with the appropriate TTL tier. The entry MUST use `cache_identity()` from CredentialIdentity, MUST NOT use `detection_identity()`.

3. **LRU Eviction**: When `max_entries` is reached, the system MUST evict the least-recently-used entry. `max_entries` MUST be configurable (default: 1000, range: 1..100,000).

4. **TTL Expiry and Invalidation**: Expired entries MUST be lazily removed on next access. When a credential enters cooldown, the system MUST bulk-invalidate all cache entries for that `credential_id`. A periodic cleanup (every 5 minutes) SHOULD reclaim memory from expired entries.

## Configuration

```json
{
  "crossRequestCache": {
    "enabled": true,
    "maxEntries": 1000,
    "defaultTtlSeconds": 300,
    "extendedTtlSeconds": 3600
  }
}
```

## Data Model

| Field | Type | Description |
|-------|------|-------------|
| cache_key | `[u8; 32]` | Domain-separated SHA-256 from `cache_identity()` |
| credential_id | `u64` | Credential identifier for bulk invalidation |
| conversation_id | `String` | Upstream conversation_id to reuse |
| created_at | `Instant` | Entry creation timestamp |
| ttl | `Duration` | Per-entry TTL (short or extended) |
| last_accessed | `Instant` | LRU ordering timestamp |

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-008 | **Hard dependency** — CredentialIdentity provides `cache_identity()` for key derivation |
| REQ-002 | **Soft dependency** — Metrics records cache hit/miss events |
| REQ-004 | **Soft dependency** — Converter injects `conversation_id` into Kiro request |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-02 | System Architect | Cache key design: `(credential_id, fingerprint_hash)` |
| PM-06 | Product Manager | Highest-ROI feature: direct upstream cost reduction |
| SME-01 | Subject Matter Expert | Domain-separated SHA-256 derivation via CredentialIdentity |
| SME-05 | Subject Matter Expert | Layer on top of CacheTracker, do not replace |
| TS-03 | Test Strategist | Deterministic tests for LRU eviction, TTL expiry, fingerprint key resolution |

## Open Questions

- Whether `conversation_id` returned by upstream is stable across retries on different credentials (see context-package.json open_questions). If unstable, cache entries MUST be scoped per-credential (current design already handles this).
- Authoritative insertion point: `stream.rs` (has conversation_id from response) vs `provider.rs` (orchestrates the call). Current recommendation: `stream.rs` for insertion, `handlers.rs` for lookup.

## State Machine

```
Empty → Active        (cache miss + successful upstream response)
Active → Active       (cache hit within TTL; does NOT refresh TTL)
Active → Expired      (TTL elapsed; lazy removal on next access)
Active → Evicted      (LRU capacity reached)
Active → Invalidated  (credential enters cooldown)
```
