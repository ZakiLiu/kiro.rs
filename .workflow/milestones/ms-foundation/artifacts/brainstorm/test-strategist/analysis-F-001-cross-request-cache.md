# F-001 — Cross-Request Prefix Cache

> Role: test-strategist | Related decisions: TS-01, TS-03, TS-10

## Architecture

The cache module test suite MUST be structured as a self-contained unit test module within the cache implementation file (following the `#[cfg(test)]` pattern used by cache_tracker.rs with its 11 existing tests). Tests MUST operate on the cache's public API without requiring network access or live credentials.

Key testable components:
- **LRU eviction logic** — Deterministic eviction order when cache reaches max_entries.
- **TTL bucket management** — Two-tier TTL (5m for normal, 1h for high-frequency) with time-controlled expiry.
- **Fingerprint-to-conversation_id mapping** — Key derivation from CredentialIdentity trait, ensuring same credential+fingerprint always resolves to same conversation_id.
- **Concurrent access** — Multiple readers/writers via `parking_lot::Mutex` (matching existing concurrency patterns in token_manager.rs).

The existing `cache_tracker.rs` (per-request SHA-256 simulation) provides a reference for how cache tests are structured in this codebase.

## Interface Contract

- `Cache::get(credential_id, fingerprint) -> Option<ConversationId>` — Lookup MUST return None for expired entries.
- `Cache::put(credential_id, fingerprint, conversation_id, ttl_tier)` — Insert MUST evict LRU when at capacity.
- `Cache::stats() -> CacheStats` — MUST report hit_count, miss_count, eviction_count, entry_count for metrics integration (see F-002).

Test doubles: No mocking framework needed. The cache is a pure data structure with `Instant`-based TTL. For time-controlled tests, SHOULD use a `Clock` trait or `tokio::time::pause()` to freeze time.

## Constraints (RFC 2119)

- Cache MUST evict least-recently-used entries when max_entries is exceeded.
- Cache MUST NOT return expired entries regardless of LRU position.
- Cache MUST produce identical keys for identical (credential_id, fingerprint) pairs across process restarts.
- Cache SHOULD support concurrent read/write without deadlock (verified by multi-threaded test).
- Cache MAY log eviction events for debugging but MUST NOT block on logging.

## Test Approach

**Unit tests (≥ 15 tests):**
1. Insert and retrieve — basic round-trip.
2. Insert beyond max_entries — verify LRU eviction order.
3. TTL expiry — insert, advance time, verify None return.
4. TTL tier differentiation — 5m vs 1h entries expire at correct times.
5. Key collision — same credential+fingerprint overwrites previous entry.
6. Different credentials — entries are isolated per credential.
7. Cache stats accuracy — hit/miss/eviction counters.
8. Empty cache — get returns None, stats show zeros.
9. Concurrent access — spawn 10 threads doing put/get, verify no panic.
10. Eviction callback — verify evicted entries are not retrievable.

**Property-based tests (proptest):**
- Invariant: after N inserts into a cache of size M (M < N), exactly M entries remain.
- Invariant: no expired entry is ever returned by get.
- Invariant: LRU order is maintained — accessing an entry moves it to most-recently-used.

**Integration tests:**
- Cache with CredentialIdentity trait (see F-008): verify fingerprint derivation produces correct cache keys.

## TODOs

- Decide on time control mechanism: `tokio::time::pause()` vs injectable `Clock` trait.
- Determine max_entries default value from design research to calibrate eviction tests.
- Create fixture for multi-credential scenarios with realistic fingerprint diversity.
