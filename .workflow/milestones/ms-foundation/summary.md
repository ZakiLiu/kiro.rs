# Milestone: ms-foundation — Foundation (MVP) v1.2.0

**Completed**: 2026-06-05
**Artifacts**: 7 (brainstorm:1, blueprint:1, analyze:1, plan:1, execute:1, verify:1, review:1)

## Key Outcomes

- **CredentialIdentity trait** (F-008): Domain-separated identity abstraction with three methods (detection_identity, cache_identity, credential_id). SHA-256 with "cache:" prefix ensures cryptographic independence between anti-detection fingerprints and cache keys.

- **CrossRequestCache** (F-001): LRU per-credential cache mapping (credential_id, content_fingerprint) → conversation_id. HashMap+VecDeque implementation avoids external dependencies. Integrated with converter via forced_conversation_id injection.

- **MetricsCollector** (F-002): Ring buffer (parking_lot::Mutex + VecDeque) recording request lifecycle events. Admin API exposes /metrics/summary, /metrics/by-model, /metrics/by-credential aggregation endpoints.

- **Zero regression**: All 415 tests pass. Anti-detection, compression, image processing, Web Portal, and CLI endpoint fully preserved.

## Learnings

- **Domain separation pattern**: Using SHA-256 with different domain prefixes from the same seed is an effective way to generate multiple non-correlatable identities. Applicable to any system needing separate derived keys.

- **Wave parallelism works**: TASK-001 (identity) and TASK-002 (metrics) had zero runtime dependency and executed cleanly in parallel. Wave 2 (cache + admin) also parallel with no merge conflicts.

- **LRU without external deps**: HashMap+VecDeque LRU is O(n) for position lookup but perfectly acceptable at max_entries=1000. Avoids adding linked-hash-map or indexmap crate.

- **Option\<Arc\<T\>\> pattern**: Feature-gated shared state via Option\<Arc\<T\>\> allows graceful disable via config while maintaining zero-cost when disabled.

## Next Milestone

**Milestone 2: Reliability (v1.3.0)** — ErrorMapper (unified error mapping with classify + to_anthropic_response + RequestContext) + Converter Enhancement (tool name shortening + forced conversation_id completion).
