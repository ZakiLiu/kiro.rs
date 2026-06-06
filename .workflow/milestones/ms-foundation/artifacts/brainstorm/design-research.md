# Design Research: kiro.rs vs kiro-rs-dev-source Codebase Comparison

## Executive Summary

Two codebases implementing Anthropic Claude API compatible proxy services for Kiro, but with fundamentally different product strategies:
- **Current (kiro.rs v1.1.31)**: "Swiss Army Knife" — scale, evasion, compression, image processing
- **Dev-Source (kiro-rs-dev-source v2026.3.1)**: "Production Platform" — administrative control, observability, caching, document handling

## Quantitative Overview

| Metric | kiro.rs (Current) | kiro-rs-dev-source |
|--------|-------------------|-------------------|
| Version | 1.1.31 (semver) | 2026.3.1 (calendar) |
| Lines of Code | ~30,557 | ~13,897 |
| Source Files | 68 .rs files | 72 .rs files |
| Author | M-JYuan (haoyue) | seven7763 |
| Repository | github.com/M-JYuan/kiro.rs | github.com/seven7763/kiro |
| TLS Strategy | Pure rustls | native-tls + rustls dual |

## Module Structure Comparison

### Current Project Modules (Monolithic Style)
- `anthropic/converter.rs` — single file (~1000+ lines)
- `anthropic/stream.rs` — single file (~500+ lines)
- `kiro/token_manager.rs` — single file (~700 lines)
- Additional unique modules: compressor.rs, tool_compression.rs, truncation.rs, cache_tracker.rs, cooldown.rs, affinity.rs, rate_limiter.rs, web_portal.rs, fingerprint.rs, background_refresh.rs, image.rs, debug.rs, token.rs

### Dev-Source Modules (Modularized Style)
- `anthropic/converter/` — 6 sub-modules (mod, model_map, session, tools, content, history)
- `anthropic/stream/` — 6 sub-modules (mod, signature, thinking, sse_state, context, buffered)
- `kiro/token_manager/` — 8 sub-modules (mod, acquire, admin_ops, failure, failure_kind, persistence, refresh, selection)
- Additional unique modules: prompt_cache.rs, prompt_filter.rs, prompt_presets.rs, document.rs, error_map.rs, models.rs, preprocess.rs, token_count.rs, cache_accounting.rs, metrics.rs

## Feature Comparison Matrix

### Current Project Only (11 unique features)
1. **Image Processing** (image.rs) — GIF frame extraction, format conversion, size constraints
2. **Input Compression Pipeline** (compressor.rs) — 5-stage: whitespace → thinking → tool_result → tool_input → history
3. **Tool Compression** (tool_compression.rs) — Tool-specific compression helpers
4. **History Truncation** (truncation.rs) — Message truncation for oversized histories
5. **Credential Affinity** (affinity.rs) — Per-credential unique fingerprint generation
6. **Cooldown Manager** (cooldown.rs) — Categorized cooldown (quota/network/rate_limit)
7. **Background Refresh** (background_refresh.rs) — Pre-emptive token refresh before expiry
8. **Web Portal Client** (web_portal.rs) — CBOR-over-HTTP RPC to app.kiro.dev
9. **Device Fingerprint** (fingerprint.rs) — Platform-aware identity simulation
10. **Rate Limiter** (rate_limiter.rs) — Daily quotas, inter-request intervals, exponential backoff
11. **Cache Tracker** (cache_tracker.rs) — Per-request prefix cache simulation (SHA-256)

### Dev-Source Only (13 unique features)
1. **Advanced Prompt Cache** (prompt_cache.rs) — Cross-request conversation_id reuse, LRU eviction, TTL buckets
2. **Prompt Filter** (prompt_filter.rs) — System prompt restriction stripping (14+ patterns)
3. **Prompt Presets** (prompt_presets.rs) — Built-in system prompt library (override/pentest/nsfw/code_complete/concise)
4. **PDF Document Handler** (document.rs) — Base64→PDF extraction via lopdf
5. **Error Mapping** (error_map.rs) — Kiro→Anthropic error translation with Retry-After
6. **Model List** (models.rs) — Dynamic model listing with -thinking variants
7. **Request Preprocessing** (preprocess.rs) — System prompt injection, thinking config normalization
8. **Token Counting** (token_count.rs) — Advanced CJK weighting, external API support
9. **Cache Accounting** (cache_accounting.rs) — Token usage breakdowns for cache operations
10. **Request Metrics** (metrics.rs) — Ring buffer, TTFB tracking, stream abort detection
11. **Admin Metrics** (admin/metrics.rs) — Windowed stats, per-model breakdowns, Prometheus-compatible
12. **Hash Utils** (common/hash.rs) — Consistent hash functions for cache keys
13. **I/O Utils** (common/io.rs) — File reading helpers

## Architectural Paradigm Comparison

| Aspect | Current (kiro.rs) | Dev-Source |
|--------|-------------------|-----------|
| Credential Mgmt | Background refresh + cooldown + affinity | Multi-credential with failure tracking + semaphores |
| Cache Strategy | Shallow per-request simulation | Deep cross-request prefix cache with LRU |
| System Prompts | Hard-coded wrapping for wire alignment | Presets + filtering + runtime UI injection |
| Compression | 5-stage pipeline (critical for 5MB limit) | Not implemented |
| Document Handling | Not implemented | PDF extraction via lopdf |
| Error Handling | Inline in handlers | Dedicated error_map module |
| Observability | None (no metrics) | Ring buffer + Admin aggregation |
| Token Counting | Basic local estimation | Advanced with external API, CJK weighting |
| Image Processing | Full pipeline (GIF extraction, format conversion) | Not implemented |
| Anti-Detection | Heavy (fingerprint, affinity, rate limiting) | Light (API compatibility focus) |

## Dependencies Diff

### Current Only
- `serde_cbor = "0.11"` — Web Portal CBOR encoding
- `image = "0.25"` — Image processing
- `uuid` with v5 feature — Deterministic agentContinuationId

### Dev-Source Only
- `regex = "1"` — Pattern matching for prompt filtering
- `lopdf = "0.32"` — PDF text extraction
- `reqwest` with extra features: `http2`, `system-proxy`, `charset`
- `tower-http` with `timeout` feature

## Strategic Assessment

**Current project strengths**: Production-hardened for scale/stealth — compression handles Kiro's 5MB limit, anti-detection mechanisms mimic real users, image processing enables multimodal use cases, Web Portal integration provides usage visibility.

**Dev-source strengths**: Developer/admin-friendly — prompt presets enable runtime behavior tuning, metrics provide operational visibility, cross-request caching reduces costs, PDF support broadens use cases.

**Neither is a superset of the other.** Optimal approach: cherry-pick best features from both.
