# Subject Matter Expert Analysis — kiro.rs vs kiro-rs-dev-source Codebase Comparison

> Contract: guidance-specification.md §6 (decisions SME-01 through SME-04)
> Owns: Domain expertise on Kiro/Anthropic protocol internals, prefix caching strategies, credential management patterns, anti-detection mechanisms, and protocol conversion edge cases
> Does not own: System architecture decomposition (SA), product prioritization (PM), test strategy details (TS)

## 1. Role Mandate

This analysis provides domain expertise on the Kiro API protocol (AWS Event Stream binary, CRC32C verification), Anthropic Messages API format (streaming SSE, tool_use/tool_result pairing), prefix caching strategies (SHA-256 fingerprinting, conversation_id mapping, LRU eviction, TTL buckets), credential management patterns (multi-token failover, cooldown classification, semaphore-based concurrency), and protocol conversion edge cases (model mapping, JSON Schema normalization, tool placeholder generation). The SME role evaluates technical merit of both codebases from a domain correctness standpoint, identifies pitfalls in feature migration, and ensures the fusion strategy preserves the current project's core competitive advantages — particularly the compression-plus-anti-detection synergy that neither codebase achieves alone. Decisions are deferred to SA for architectural decomposition and to PM for business prioritization.

## 2. Decision Digest

### Decisions

| ID | Feature | Stance | Constraints (RFC 2119) |
|----|---------|--------|------------------------|
| SME-01 | F-001, F-008 | Reuse existing SHA-256 fingerprint as cache key via CredentialIdentity trait; domain-separated derivation prevents anti-detection correlation | MUST use domain-separated SHA-256; MUST NOT share raw fingerprint between detection and caching |
| SME-02 | F-007 | Current separation (token_manager + cooldown + rate_limiter as peer files) is superior to dev-source's nested approach | MUST preserve current organizational separation; SHOULD split only converter.rs and stream.rs |
| SME-03 | F-004 | Tool name shortening and forced conversation_id are both domain-justified enhancements | MUST implement deterministic shortening; MUST inject conversation_id when cache provides it |
| SME-04 | Cross-cutting | Anti-detection (fingerprint + affinity + rate_limiter) is the core competitive moat; all new features must preserve it | MUST retain and strengthen; MUST assess anti-detection impact for every new feature |
| SME-05 | F-001 | CacheTracker is more sophisticated than dev-source prompt_cache; cross-request cache must layer on top, not replace | MUST NOT modify existing CacheTracker; MUST consume its profile output |
| SME-06 | F-003 | Five distinct Kiro error categories require unified mapping to Anthropic error taxonomy | MUST produce spec-compliant Anthropic error JSON; MUST inject Retry-After headers |
| SME-07 | F-002 | Observability is the most critical gap; five domain signals must be tracked | MUST cover latency, TTFB, cache hit rate, compression ratio, error distribution |
| SME-08 | F-005 | Prompt presets are valuable; prompt filter requires independent security review | SHOULD defer filter to separate review cycle; MUST make presets configurable via Admin API |
| SME-09 | F-006 | PDF support follows existing image processing pattern; must be feature-gated | MUST feature-gate lopdf dependency; MUST NOT crash on extraction failures |

### Interfaces

> **Cross-Role Resolution (C-003)**: ErrorMapper consumers expanded to handlers.rs + stream.rs + provider.rs (union of SA and SME lists).

> **Cross-Role Gap (G-003)**: SA lists stream.rs (not provider.rs) as the cache insertion consumer. Resolve with SA on where conversation_id extraction and cache insert should occur.

| Name | Contract | Consumers |
|------|----------|-----------|
| CredentialIdentity trait | `detection_identity() -> &Fingerprint`, `cache_identity() -> [u8; 32]`, `credential_id() -> u64` | provider.rs, cross_request_cache.rs, affinity.rs |
| CrossRequestCache | `lookup(CacheKey) -> Option<String>`, `insert(CacheKey, conversation_id, ttl)` | handlers.rs, provider.rs |
| ToolNameMap | `shorten(name) -> String`, `restore(short) -> Option<&str>` | converter.rs, stream.rs |
| MetricsCollector | `record(RequestMetric)`, `query(window) -> MetricsSummary` | handlers.rs, admin/handlers.rs |
| error_map | `map_upstream_error(status, body, was_compressed) -> AnthropicError` | handlers.rs, provider.rs |

### Cross-Cutting Positions

| Topic | Stance |
|-------|--------|
| Compression pipeline preservation | The 5-stage pipeline is a non-negotiable competitive advantage; all feature additions MUST be evaluated for compression interaction |
| Anti-detection integrity | Fingerprint diversity, affinity binding, and rate limiting form an interlocking system; feature migration MUST NOT create new correlation vectors |
| Cache key stability | Prefix fingerprints MUST be deterministic and stable across identical request content; billing header canonicalization is critical |
| Domain-separated identity | Cache and detection identities MUST be cryptographically independent to prevent upstream correlation attacks |
| Reimplementation over transplant | Per TS-02, features SHOULD be reimplemented referencing dev-source design, not ported code — the codebases have fundamentally different module styles |

### Findings Summary

| Slug | Title | Impact |
|------|-------|--------|
| compression-anti-detection-synergy | Compression and Anti-Detection Synergy Creates Unique Moat | HIGH |
| cache-tracker-reuse | Existing CacheTracker is More Sophisticated Than Dev-Source's prompt_cache | MEDIUM |

## 3. Cross-Cutting Foundations

### Pitfall Taxonomy

> **Cross-Role Synergy (S-001)**: Aligns with SA ErrorMapper "BadRequest" classification — compression-induced 400s will be automatically detected via RequestContext.was_compressed parameter in the unified error_map.

> **Cross-Role Synergy (S-002)**: Aligns with TS integration test priority — pitfall severity ratings directly inform test-strategist's risk-based test ordering (finding: integration-test-absence).

| Pitfall | Trigger | Severity | Mitigation |
|---------|---------|----------|------------|
| Cache-detection correlation | Sharing raw fingerprint between cache lookup and User-Agent headers | HIGH | Domain-separated SHA-256 derivation via CredentialIdentity trait (see SME-01) |
| Conversation_id temporal fingerprint | Reusing conversation_id across requests creates detectable session patterns | HIGH | Limit conversation_id reuse to same-credential, same-model scope; enforce TTL expiry |
| Compression-induced 400 errors | Aggressive truncation produces malformed tool_use/tool_result pairs | MEDIUM | Post-compression validation pass (existing `fix_tool_pairing` logic); error_map diagnostic logging |
| Tool name collision | SHA-256 truncation to 8 hex chars creates theoretical collision space of 2^32 | LOW | Monitor collision rate in metrics; fall back to 12 hex chars if collisions detected |
| LRU cache memory pressure | 10,000 entries with large conversation_id strings could consume significant heap | MEDIUM | Set max_entries based on measured entry size; add memory usage to metrics dashboard |
| Prefix cache key drift | Billing header changes between requests invalidate otherwise-identical cache fingerprints | MEDIUM | Already mitigated by existing `canonicalize_system_block_for_cache()` in cache_tracker.rs |

### Pattern Fingerprints

The Kiro API proxy domain has several distinctive patterns that distinguish it from generic API proxies:

1. **Binary protocol asymmetry**: Requests are JSON-over-HTTPS but responses are AWS Event Stream binary (headers + payload + CRC32C checksum). This asymmetry means request-side optimizations (compression, tool shortening) operate on JSON, while response-side processing (stream parsing, event conversion) operates on binary frames. These two paths have fundamentally different performance characteristics and failure modes.

2. **Prefix caching economics**: Anthropic charges for cache creation tokens at a premium but provides cache read tokens at a discount. The per-request `CacheTracker` simulates this economics to report estimated costs. Cross-request caching (F-001) MUST integrate with this economics model to provide accurate cost reporting.

3. **Deterministic fingerprint seeding**: The current project uses `refresh_token` or `machine_id` as the fingerprint seed, ensuring the same credential always produces the same simulated device identity. This determinism is critical — random fingerprints per request would be easily detected as non-human behavior by upstream anomaly detection.

4. **Cooldown category hierarchy**: The cooldown system (`cooldown.rs`) distinguishes between rate limits (short, auto-recoverable), server errors (medium, retry-worthy), and model unavailability (long, global circuit breaker). This hierarchy prevents a single-credential rate limit from cascading into service-wide unavailability.

### Domain-Silence Decisions

Several areas where the SME analysis deliberately does not take a position, because the evidence is insufficient or the decision belongs to another role:

1. **Prompt filter security implications**: The dev-source's prompt_filter.rs strips safety instructions from system prompts. The SME has deep understanding of what this does technically but defers the ethical and security evaluation to a dedicated review process (see PM-05).

2. **Token counting accuracy**: The dev-source implements CJK-weighted token counting and external API fallback. The current project uses local estimation. The accuracy difference is real but quantifying its impact on cost reporting requires production traffic analysis that is not available.

3. **TLS strategy**: The current project uses pure `rustls` while the dev-source uses `native-tls + rustls` dual. The TLS strategy choice is a deployment concern, not a domain concern.

### Differentiation Thesis

The current project (kiro.rs) differentiates through operational resilience — its compression pipeline, anti-detection system, and credential management work together to maintain service availability under adversarial conditions (upstream rate limiting, payload size limits, anomaly detection). The dev-source differentiates through administrative control — presets, filters, and metrics provide operators with visibility and tuning levers.

The fusion strategy aims to combine both differentiation axes: operational resilience as the foundation, administrative control as the overlay. This ordering is deliberate — admin features that compromise operational resilience (e.g., prompt filter creating detectable patterns) MUST NOT be adopted without mitigation.

### Crosswalk

| Dev-Source Module | Current Project Equivalent | Gap Analysis |
|-------------------|---------------------------|--------------|
| prompt_cache.rs | cache_tracker.rs | Current is more sophisticated per-request; dev-source adds cross-request conversation_id reuse |
| error_map.rs | Inline in provider.rs/handlers.rs | Current has no centralized error mapping; dev-source design is better |
| metrics.rs + admin/metrics.rs | None | Current has zero observability; full gap |
| prompt_presets.rs | None | Current has no runtime prompt configuration; full gap |
| prompt_filter.rs | None | Current has no prompt filtering; requires security review before adoption |
| document.rs | None | Current has no PDF support; image.rs provides the pattern for integration |
| converter/ (6 sub-modules) | converter.rs (monolithic) | Dev-source is more modular; current has better Schema normalization and wire alignment |
| token_manager/ (8 sub-modules) | token_manager.rs + cooldown.rs + rate_limiter.rs + affinity.rs + fingerprint.rs + background_refresh.rs | Current separation is actually superior — clearer responsibility boundaries |
| common/hash.rs | sha2 usage in fingerprint.rs, cache_tracker.rs | Current uses sha2 crate directly; no abstraction gap |

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
| [findings-compression-anti-detection-synergy.md](findings-compression-anti-detection-synergy.md) | finding | — | Description, Affected Features, Recommendation |
| [findings-cache-tracker-reuse.md](findings-cache-tracker-reuse.md) | finding | — | Description, Affected Features, Recommendation |

## 5. Outstanding TODOs

- **Codebase study**: Audit all `Fingerprint::generate_from_seed()` call sites to plan CredentialIdentity migration order.
- **Codebase study**: Count exact line counts and public function inventory for converter.rs and stream.rs to validate refactoring split dimensions.
- **Codebase study**: Catalog all inline error handling paths in provider.rs and handlers.rs for error_map coverage verification.
- **External research**: Study Anthropic API error documentation for the complete error type taxonomy and status code semantics.
- **External research**: Evaluate `lopdf` vs `pdf-extract` crate for text extraction quality on complex PDF layouts.
- **Decision deferred**: Prompt filter (prompt_filter.rs) security review — requires dedicated assessment before adoption decision.
- **Decision deferred**: Token counting accuracy impact — requires production traffic analysis to quantify cost reporting delta.
- **Measurement needed**: Memory footprint of 10,000 CrossRequestCache entries to validate default max_entries.
- **Measurement needed**: `record()` latency for ring buffer metrics under concurrent load (target < 500ns).
- **Design decision**: Whether conversation_id reuse TTL should match CacheTracker TTL or use independent configuration.
