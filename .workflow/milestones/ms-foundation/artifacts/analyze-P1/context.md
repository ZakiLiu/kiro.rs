# Context: Phase 01 — Foundation Infrastructure

**Date**: 2026-06-05
**Scope**: micro (Phase 1 of Milestone ms-foundation)
**Areas discussed**: CredentialIdentity trait design, CrossRequestCache topology & insertion point, MetricsCollector architecture, conversation_id stability, cache key isolation

## Decisions

### Decision 1: CredentialIdentity Trait Signature
- **Context**: Need unified fingerprint abstraction for anti-detection AND cache; SA proposed 2-method, SME proposed 3-method
- **Options**:
  1. Two-method: identity_key() + derived_cache_key() (SA)
  2. Three-method: detection_identity() + cache_identity() + credential_id() (SME)
- **Chosen**: Three-method (SME) — **Reason**: Proper domain separation, preserves existing credential_id usage, cryptographic independence via domain-separated SHA-256
- **Impact**: F-008 implementation, all downstream consumers

### Decision 2: Cache Key Isolation Strategy (OQ-1 resolved)
- **Context**: conversation_id may differ across credential retries; need to decide cache key scope
- **Options**:
  1. Per-credential isolation: cache key = (credential_id, content_fingerprint)
  2. Shared across credentials: cache key = content_fingerprint only
  3. Experiment first, decide later
- **Chosen**: Per-credential isolation — **Reason**: Most safe; different credentials may get different conversation_ids from upstream; avoids cross-credential cache pollution
- **Impact**: Cache hit rate is per-credential (slightly lower than shared), but zero risk of stale/wrong conversation_id injection

### Decision 3: Cache Insertion Point (OQ-3 resolved)
- **Context**: Where to do cache lookup and insert — stream.rs, provider.rs, or handlers.rs
- **Options**:
  1. handlers.rs unified management (lookup before convert, insert after response)
  2. stream.rs does insert (tighter coupling)
  3. provider.rs does insert (requires conversation_id passthrough)
- **Chosen**: handlers.rs unified — **Reason**: Handler already orchestrates full request lifecycle; lookup at line ~1089 (before convert_request), insert at line ~1406/1721 (after response); clean separation from stream/provider
- **Impact**: handlers.rs gains cache dependency; stream.rs needs to expose conversation_id from InitialResponse event

### Decision 4: ErrorMapper Signature (Cross-Role Resolution C-002)
- **Context**: Three roles proposed incompatible signatures
- **Chosen**: SA dual-function (classify + to_anthropic_response) with RequestContext struct carrying was_compressed + upstream_headers
- **Impact**: Deferred to Milestone 2 (EPIC-002), but contract defined now

### Decision 5: convert_request() Signature Change
- **Context**: Need to inject forced_conversation_id from cache
- **Chosen**: Add `forced_conversation_id: Option<String>` parameter to convert_request()
- **Reason**: Minimal invasion; when None, behavior identical to current; when Some, skips conversation_id derivation at converter.rs:478-484
- **Impact**: All call sites of convert_request() must pass the new parameter

## Constraints

### Locked

1. **CredentialIdentity: three-method trait** — detection_identity() → &Fingerprint, cache_identity() → [u8;32], credential_id() → u64. Domain-separated SHA-256 derivation. MUST NOT share raw fingerprint between detection and cache. (Source: brainstorm C-001, SME-01, SA-04)

2. **Cache key: per-credential isolation** — cache key = (credential_identity.cache_identity(), content_fingerprint). Conversation_id cached per-credential, not shared. (Source: OQ-1 resolution)

3. **Cache insertion: handlers.rs** — Lookup before convert_request() (~line 1089). Insert after response completion (~line 1406 stream / ~1721 non-stream). Handler extracts conversation_id from stream InitialResponse event. (Source: OQ-3 resolution)

4. **convert_request() signature** — Add `forced_conversation_id: Option<String>` parameter. When Some, use it; when None, derive as before. (Source: codebase exploration)

5. **AppState extensions** — Add `cross_request_cache: Arc<CrossRequestCache>` and `metrics: Arc<MetricsCollector>` to AppState struct in middleware.rs:68. Initialize in main.rs. (Source: codebase exploration)

6. **Config extensions** — Add to Config struct in config.rs: `cross_request_cache_enabled: bool` (default true), `cross_request_cache_max_entries: usize` (default 1000), `metrics_enabled: bool` (default true), `metrics_ring_buffer_size: usize` (default 10000). (Source: SA analysis)

7. **Preserve existing systems** — MUST NOT modify anti-detection (fingerprint, affinity, rate_limiter), compression pipeline, image processing, Web Portal, or CLI endpoint behavior. (Source: brainstorm C-001, SA-11)

8. **Credential management split** — MUST preserve current split architecture (token_manager + cooldown + rate_limiter + affinity + background_refresh as separate files). (Source: brainstorm C-002, SME-02)

### Free

9. **CacheTracker integration** — Implementer MAY choose whether CrossRequestCache reuses CacheTracker's content fingerprint or computes its own. Research suggests: layering on top of CacheTracker (reuse its SHA-256 profile) is simpler and avoids duplicated hashing.

10. **MetricsCollector concurrency** — Implementer MAY use parking_lot::Mutex, channel-based, or lock-free ring buffer. Research suggests: parking_lot::Mutex with short critical sections (no I/O inside lock) is consistent with existing codebase patterns.

11. **Admin metrics endpoint grouping** — Implementer MAY expose metrics as separate endpoints (/metrics/summary, /metrics/by-model, /metrics/by-credential) or a single endpoint with query params. Research suggests: separate endpoints match existing Admin API pattern (each handler is a distinct function in admin/handlers.rs).

12. **New file placement** — CredentialIdentity trait in `src/kiro/identity.rs`, CrossRequestCache in `src/anthropic/cross_request_cache.rs`, MetricsCollector in `src/metrics.rs` or `src/anthropic/metrics.rs`. Implementer's choice on exact module structure.

### Deferred

13. **Prompt filter adoption** — Deferred to separate security review. Not in any current milestone. (Source: brainstorm PM-05)

14. **Token counting accuracy** — Dev-source has CJK-weighted token counting; accuracy delta requires production traffic analysis. Deferred to post-MVP evaluation. (Source: SME domain-silence)

15. **Distributed cache** — Out of scope; single-process in-memory cache only. If horizontal scaling needed, revisit. (Source: SA boundary scenarios)

## Code Context

### Key Integration Points (from codebase exploration)

| Feature | File | Line | Action |
|---------|------|------|--------|
| F-008 | kiro/model/credentials.rs | struct KiroCredentials | impl CredentialIdentity |
| F-008 | kiro/fingerprint.rs | ~100 | Fingerprint::generate_from_seed — used by detection_identity() |
| F-001 | anthropic/converter.rs | 411 | MODIFY: add forced_conversation_id param |
| F-001 | anthropic/converter.rs | 478-484 | conversation_id derivation — skip when forced |
| F-001 | anthropic/handlers.rs | ~1089 | INSERT: cache lookup before convert |
| F-001 | anthropic/handlers.rs | ~1406 | INSERT: cache record after stream |
| F-001 | anthropic/handlers.rs | ~1721 | INSERT: cache record after non-stream |
| F-001 | anthropic/stream.rs | 625 | Event::InitialResponse { conversation_id } — extract for cache |
| F-001 | anthropic/middleware.rs | 68 | ADD: cross_request_cache to AppState |
| F-001 | model/config.rs | 17 | ADD: cache config fields |
| F-002 | anthropic/handlers.rs | 1006-1014 | INSERT: emit request_received metric |
| F-002 | anthropic/handlers.rs | 1270/1435 | INSERT: emit credential_selected metric |
| F-002 | anthropic/handlers.rs | 1322/1721 | INSERT: emit request_completed metric |
| F-002 | anthropic/middleware.rs | 68 | ADD: metrics to AppState |
| F-002 | admin/handlers.rs | ~160+ | ADD: metrics query endpoints |
| F-002 | admin/service.rs | ~100 | ADD: metrics query methods |
| F-002 | main.rs | 206 | MODIFY: pass metrics to router |

### Existing Patterns to Follow

- **AppState extension**: Use `Arc<T>` for shared state (see existing `kiro_provider: Option<Arc<KiroProvider>>`)
- **Config deserialization**: Use `#[serde(default)]` for backward-compatible config fields
- **Admin API auth**: Existing `adminApiKey` check pattern in admin/middleware.rs
- **Thread safety**: parking_lot::Mutex/RwLock throughout codebase (not std::sync)
- **Logging**: tracing crate with structured fields
