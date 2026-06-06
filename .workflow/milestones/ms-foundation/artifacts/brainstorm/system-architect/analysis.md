# System Architect Analysis -- kiro.rs vs kiro-rs-dev-source Codebase Fusion

> Contract: guidance-specification.md S4 (decisions SA-01..SA-06)
> Owns: module boundaries, data flow topology, state management, error taxonomy, trait abstractions, observability integration points
> Does not own: product prioritization (PM), domain-specific anti-detection heuristics (SME), test coverage thresholds (TS), UI/UX (ui-designer)

## 1. Role Mandate

The system architect defines the structural skeleton into which dev-source features are absorbed. The current kiro.rs architecture is a single-binary monolith (Axum + Tokio) with a clear layered request flow: HTTP ingress, Anthropic compatibility layer, protocol converter, Kiro provider, and credential management. The fusion challenge is to graft six new capabilities (cross-request cache, metrics, error mapping, converter enhancements, prompt presets, PDF support) onto this skeleton without destabilizing the production-hardened anti-detection and compression pipelines that constitute the project core competitive advantage. All architectural decisions defer product scope to PM (see PM-01..PM-05) and domain heuristics to SME (see SME-01..SME-04). This analysis focuses on module topology, data model, state machines, trait abstractions, and integration seams.

## 2. Decision Digest

### Decisions

| ID | Feature | Stance | Constraints (RFC 2119) |
|----|---------|--------|------------------------|
| SA-01 | F-007 module-refactor | Incremental decomposition of largest files first (converter.rs, token_manager.rs, stream.rs) | SHOULD split files exceeding 500 LOC into sub-modules; MUST NOT break public API surface during refactor |
| SA-02 | F-001 cross-request-cache | Cross-request conversation_id reuse keyed on (credential_id, fingerprint_hash) | MUST reuse existing fingerprint infrastructure via CredentialIdentity trait (see SA-04) |
| SA-03 | F-003 error-mapping | Dedicated error_map module translating upstream Kiro errors to Anthropic error format | MUST classify errors as retryable vs terminal; MUST inject Retry-After headers for 429/529 |
| SA-04 | F-008 shared-identity | CredentialIdentity trait unifying fingerprint usage across affinity and cache | MUST define a single trait with identity_key() and derived_cache_key() methods |
| SA-05 | cross-cutting | Credential management remains split (token_manager + cooldown + rate_limiter) | MUST preserve current separation of concerns |
| SA-06 | cross-cutting | Input compression pipeline preserved as-is | MUST NOT remove or weaken compression; new error_map MUST handle post-compression errors |
| SA-07 | F-002 request-metrics | Ring-buffer metrics collection with Admin API exposure | MUST track latency, TTFB, cache hit rate, credential distribution, error classification |
| SA-08 | F-004 converter-enhance | Tool name shortening and forced conversation_id injection | MUST maintain reversible name mapping; SHOULD store mapping in request-scoped context |
| SA-09 | F-005 prompt-presets | Configurable system prompt library with runtime switching | SHOULD store presets in config.json; MUST NOT modify presets without admin authorization |
| SA-10 | F-006 pdf-support | PDF text extraction via lopdf integrated into content block processing | SHOULD add lopdf dependency; MUST handle malformed PDFs gracefully without panicking |

### Interfaces

> **Cross-Role Resolution (C-003)**: ErrorMapper consumers expanded to handlers.rs + stream.rs + provider.rs (union of SA and SME lists).

> **Cross-Role Gap (G-003)**: SME lists provider.rs as CrossRequestCache consumer for cache insertion; SA lists stream.rs. Clarify authoritative insertion point — stream.rs (has conversation_id) or provider.rs (orchestrates the call).

| Name | Contract | Consumers |
|------|----------|-----------|
| CredentialIdentity trait | fn identity_key() -> [u8; 32] + fn derived_cache_key() -> [u8; 32] | affinity, cross-request-cache, rate_limiter |
| ErrorMapper | fn map_upstream_error(status: u16, body: &[u8]) -> AnthropicError | handlers.rs, stream.rs |
| MetricsCollector | fn record(event: RequestEvent) + fn query(window: TimeWindow) -> MetricsSnapshot | handlers.rs, admin/handlers.rs |
| PromptPresetStore | fn get(name) -> Option SystemMessage list + fn list() -> PresetMeta list | converter.rs, admin/handlers.rs |
| PdfExtractor | fn extract_text(data: &[u8]) -> Result String | converter.rs content block processing |
| ErrorMapper -> MetricsCollector | error_map calls MetricsCollector::record() for error classification counting | metrics ring buffer |
| CrossRequestCache | fn lookup(key: CacheKey) -> Option String + fn insert(key: CacheKey, conversation_id: String) | handlers.rs (pre-request), stream.rs (post-response) |

### Cross-Cutting Positions

| Topic | Stance |
|-------|--------|
| Data Model | Six new data entities (CacheEntry, MetricRecord, ErrorMapping, ToolNameMap, PromptPreset, PdfContent) all stored in-memory with optional persistence |
| State Machine | CacheEntry lifecycle: Empty -> Active -> Expired -> Evicted; credential cooldown state machine unchanged |
| Error Handling | Centralized error_map replaces inline error translation scattered across handlers and stream |
| Observability | Ring-buffer metrics with configurable window sizes; no external dependency (Prometheus-compatible via Admin API) |
| Configuration | All new features configurable via config.json with serde defaults; hot-reload via Admin API |
| Boundary Scenarios | LRU eviction under memory pressure; graceful degradation when cache is cold; PDF size limits |

### Findings Summary

| Slug | Title | Impact |
|------|-------|--------|
| cache-topology | Cache layer sits between handler and provider, orthogonal to existing cache_tracker | HIGH -- architectural seam decision affects all cache-related features |
| error-flow-gap | Current error handling is scattered across 4+ files with inconsistent mapping | HIGH -- unified error_map eliminates redundant translation code |

## 3. Cross-Cutting Foundations

### Data Model

Six core entities introduced by the fusion:

1. **CacheEntry** -- credential_id: u64, cache_key: [u8; 32], conversation_id: String, created_at: Instant, ttl: Duration. Keyed by (credential_id, derived_cache_key). LRU eviction with configurable max_entries (default 1000). MUST use parking_lot::Mutex for thread-safe access consistent with existing patterns.

2. **MetricRecord** -- timestamp: Instant, latency_ms: u64, ttfb_ms: Option of u64, credential_id: u64, model: String, status: u16, cache_hit: bool, error_class: Option of ErrorClass. Stored in a fixed-size ring buffer (default 10000 entries). MUST NOT allocate unbounded memory.

3. **ErrorMapping** -- upstream_status: u16, upstream_error_type: String, anthropic_status: u16, anthropic_error_type: String, retryable: bool, retry_after_seconds: Option of u32. Static lookup table, loaded at startup. SHOULD be extensible via config for custom mappings.

4. **ToolNameMap** -- original_name: String, shortened_name: String. Request-scoped, created during converter processing and used during response reconstruction. MUST be reversible.

5. **PromptPreset** -- name: String, description: String, system_messages: Vec of SystemMessage, is_default: bool. Stored in config.json under promptPresets key. SHOULD support at most 20 presets.

6. **PdfContent** -- Ephemeral, not stored. text: String, page_count: usize. Extracted inline during content block processing. MUST cap extraction at 100 pages to prevent memory exhaustion.

### State Machine

**CacheEntry Lifecycle:**

```
                 +----------+
                 |  Empty   |  (no entry for this key)
                 +----+-----+
                      | cache miss + successful request
                      v
                 +----------+
        +--------|  Active   |--------+
        |        +----+-----+        |
        |             |               |
        |  TTL        | LRU           | credential
        |  expires    | eviction      | disabled
        |             |               |
        v             v               v
   +----------+ +----------+ +-----------+
   | Expired  | | Evicted  | |Invalidated|
   +----------+ +----------+ +-----------+
```

| From | To | Trigger | Action |
|------|----|---------|--------|
| Empty | Active | Cache miss + upstream returns conversation_id | Insert entry with TTL |
| Active | Active | Cache hit within TTL | Return conversation_id, do NOT refresh TTL (aligns with upstream behavior) |
| Active | Expired | TTL elapsed | Remove on next access (lazy cleanup) |
| Active | Evicted | LRU capacity reached | Remove oldest entry |
| Active | Invalidated | Credential disabled via cooldown | Bulk remove all entries for credential_id |

**Credential State Machine (existing, unchanged per SA-05):**

```
Available -> Cooling (failure) -> Available (recovered)
Available -> Disabled (balance/quota) -> Available (admin reset)
```

MUST NOT modify the existing credential state transitions. New cache invalidation hooks into the report_failure path.

### Error Handling Strategy

**Classification:**

| Class | HTTP Status | Retryable | Examples |
|-------|-------------|-----------|----------|
| RateLimit | 429 | Yes (with Retry-After) | Upstream 429, throttling |
| Overloaded | 529 | Yes (with backoff) | Upstream 529, service overload |
| BadRequest | 400 | No | Malformed request, schema violation |
| AuthFailure | 401/403 | No (credential-level) | Token expired, insufficient permissions |
| NotFound | 404 | No | Unknown model, invalid endpoint |
| ServerError | 500 | Yes (limited) | Upstream 500, transient failures |
| NetworkError | 502 | Yes | Connection reset, DNS failure, timeout |

**Recovery Mechanisms:**
- Retryable errors trigger credential failover (existing behavior, see SA-05)
- Error mapping MUST run after compression (see SA-06) and after streaming response parsing
- The ErrorMapper MUST produce a valid Anthropic API error response body with type, message, and optional retry_after fields
- SHOULD log original upstream error details at DEBUG level for troubleshooting

### Observability

| # | Metric/Event | Type | Source |
|---|-------------|------|--------|
| 1 | request_latency_ms | histogram | handlers.rs -- total request duration |
| 2 | ttfb_ms | histogram | stream.rs -- time to first byte in streaming responses |
| 3 | cache_hit_rate | gauge | CrossRequestCache -- hits / (hits + misses) per window |
| 4 | credential_usage_distribution | counter per credential_id | provider.rs -- requests dispatched per credential |
| 5 | error_class_count | counter per ErrorClass | ErrorMapper -- classified error occurrences |
| 6 | compression_ratio | gauge | compressor.rs -- original_size / compressed_size |
| 7 | active_cache_entries | gauge | CrossRequestCache -- current entry count |
| 8 | stream_abort_count | counter | stream.rs -- client disconnections mid-stream |

Log events MUST use structured tracing (existing tracing crate). Health check endpoint (GET /health) SHOULD return status, credentials_available count, and cache_entries count.

### Configuration

> **Cross-Role Synergy (S-003)**: Aligns with PM Feature Toggle Strategy — per-feature enabled:bool fields serve PM's dual-persona (Power User / Platform Operator) toggle requirements.

All new features MUST be configurable via config.json with serde defaults:

```rust
// Additions to existing Config struct
pub cross_request_cache: CrossRequestCacheConfig, // default enabled
pub metrics: MetricsConfig,                        // default enabled
pub prompt_presets: Vec<PromptPreset>,             // default empty
pub pdf_support_enabled: bool,                     // default true

pub struct CrossRequestCacheConfig {
    pub enabled: bool,             // default: true
    pub max_entries: usize,        // default: 1000
    pub default_ttl_seconds: u64,  // default: 300
    pub extended_ttl_seconds: u64, // default: 3600
}

pub struct MetricsConfig {
    pub enabled: bool,             // default: true
    pub ring_buffer_size: usize,   // default: 10000
    pub window_seconds: u64,       // default: 300
}
```

Validation: max_entries MUST be > 0 and <= 100_000. ring_buffer_size MUST be > 0 and <= 1_000_000. TTL values MUST be > 0.

### Boundary Scenarios

**Concurrency:** All new shared state (cache, metrics) MUST use parking_lot::Mutex or parking_lot::RwLock consistent with existing codebase patterns. Critical sections MUST be kept short (no I/O inside locks).

**Rate Limiting:** Cross-request cache lookup adds negligible latency (in-memory HashMap). MUST NOT introduce additional network calls in the hot path.

**Shutdown:** Cache entries are ephemeral and do not require persistence on shutdown. Metrics ring buffer MAY be flushed to log at shutdown for post-mortem analysis.

**Cleanup:** LRU eviction handles memory bounds. Lazy expiration (check on access) avoids background timer overhead. SHOULD run periodic bulk cleanup every 5 minutes to reclaim memory from expired entries.

**Scalability:** Single-process design (no distributed cache). The cache is per-process; horizontal scaling requires independent cache instances. This is acceptable for the proxy use case.

**Disaster Recovery:** Cache loss on restart is acceptable (cold start). Credentials and config are persisted to disk. MUST NOT store sensitive data (tokens, keys) in the cache or metrics stores.

## 4. File Index

| File | Type | Feature | Headings |
|------|------|---------|----------|
| [analysis-F-001-cross-request-cache.md](analysis-F-001-cross-request-cache.md) | feature | F-001 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [analysis-F-002-request-metrics.md](analysis-F-002-request-metrics.md) | feature | F-002 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [analysis-F-003-error-mapping.md](analysis-F-003-error-mapping.md) | feature | F-003 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [analysis-F-004-converter-enhance.md](analysis-F-004-converter-enhance.md) | feature | F-004 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [analysis-F-005-prompt-presets.md](analysis-F-005-prompt-presets.md) | feature | F-005 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [analysis-F-006-pdf-support.md](analysis-F-006-pdf-support.md) | feature | F-006 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [analysis-F-007-module-refactor.md](analysis-F-007-module-refactor.md) | feature | F-007 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [analysis-F-008-shared-identity.md](analysis-F-008-shared-identity.md) | feature | F-008 | Architecture, Interface Contract, Constraints, Test Approach, TODOs |
| [findings-cache-topology.md](findings-cache-topology.md) | finding | -- | Description, Affected Features, Recommendation |
| [findings-error-flow-gap.md](findings-error-flow-gap.md) | finding | -- | Description, Affected Features, Recommendation |

## 5. Outstanding TODOs

- Study dev-source prompt_cache.rs implementation details for TTL bucket strategy differences
- Evaluate lopdf crate security posture and fuzzing coverage before adding as dependency
- Profile memory impact of 10000-entry metrics ring buffer under sustained load
- Determine whether ErrorMapper static table or runtime-configurable mapping better fits operational needs
- Investigate whether conversation_id returned by upstream is stable across retries on different credentials
- Validate that derived_cache_key() produces sufficient entropy to prevent cross-user cache collisions
- Assess whether prompt preset hot-reload via Admin API requires file watcher or polling
