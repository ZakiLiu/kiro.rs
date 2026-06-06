---
document: product-brief
session_id: BLP-kiro-fusion-2026-06-05
version: 1.0
status: draft
spec_type: service
---

# Product Brief: kiro.rs Fusion

> Fusing operational resilience with administrative control into a single production-grade proxy.

## 1. Executive Summary

kiro.rs is a Rust-based proxy that translates Anthropic Claude API requests into Kiro (AWS CodeWhisperer) API requests, enabling Claude-compatible tooling to operate against Kiro infrastructure. The **Fusion** initiative merges capabilities from two divergent codebases — kiro.rs v1.1.31 (production, resilience-focused) and kiro-rs-dev-source v2026.3.1 (development, observability-focused) — into a unified system that serves both stealth-focused power users and visibility-focused platform operators.

The project integrates cross-request caching, request metrics, unified error mapping, and converter enhancements from the dev-source branch while preserving all existing competitive advantages: the 5-stage compression pipeline, anti-detection subsystem, GIF frame extraction, and Web Portal CBOR integration.

## 2. Vision

**One proxy, two personas, zero compromise.**

kiro.rs Fusion MUST deliver operational resilience (compression, anti-detection, image processing) and administrative control (metrics, caching, error mapping) in a single binary. Users SHOULD NOT need to choose between stealth and observability — the system MUST support both modes simultaneously, configured per-deployment.

## 3. Problem Statement

### 3.1 Current Gaps in kiro.rs v1.1.31

| Gap | Impact | Ref |
|-----|--------|-----|
| No cross-request caching | Redundant upstream calls for repeated conversation turns; higher latency and cost | SA-01 |
| No request metrics/observability | Operators cannot monitor TTFB, error rates, or usage patterns | SA-02 |
| No unified error mapping | Raw Kiro errors leak to clients, breaking Anthropic API contract | SA-03 |
| No PDF document support | Users cannot send PDF attachments via the proxy | SME-03 |
| No prompt presets | Common system prompts must be repeated in every request | PM-03 |

### 3.2 Unique Strengths to Preserve

The following capabilities exist in kiro.rs v1.1.31 but NOT in kiro-rs-dev-source:

- **5-stage compression pipeline** (`compressor.rs`): whitespace compression, thinking truncation, tool_result truncation, tool_use input truncation, history truncation — with automatic tool_use/tool_result pair repair (SA-11, C-001)
- **Anti-detection subsystem**: fingerprint rotation, session affinity, per-credential rate limiting (C-001)
- **GIF frame extraction**: adaptive sampling (max 20 frames, max 5fps) with JPEG re-encoding (C-001)
- **Web Portal CBOR integration**: binary protocol support for the Kiro Web Portal endpoint (SA-11)
- **CLI endpoint** (`kiro/endpoint/cli.rs`): dedicated endpoint for CLI-based Kiro access (SA-11)
- **Credential-level proxy support**: per-credential HTTP/SOCKS5 proxy with cached HTTP clients (C-002)

### 3.3 Why Now

The two codebases are diverging. Every week of independent development increases merge cost. The dev-source project has stabilized its cache and metrics APIs, making integration feasible. Delaying further risks architectural incompatibility.

## 4. Target Users

### 4.1 Persona A: Power User (Stealth-Focused)

- **Profile**: Individual developer or small team running a single kiro.rs instance
- **Primary needs**: Low detection risk, minimal upstream bandwidth, high reliability
- **Key features**: Compression pipeline, anti-detection (fingerprint/affinity/rate-limiter), credential failover, image processing
- **Deployment**: Single binary on a VPS or local machine
- **Success criteria**: Uninterrupted service with minimal upstream footprint

### 4.2 Persona B: Platform Operator (Visibility-Focused)

- **Profile**: Team or organization running kiro.rs as shared infrastructure
- **Primary needs**: Observability, cost tracking, error transparency, administrative control
- **Key features**: Request metrics, cross-request cache, error mapping, prompt presets, Admin UI
- **Deployment**: Shared service behind a load balancer or reverse proxy
- **Success criteria**: Full visibility into request lifecycle, predictable error behavior, reduced operational cost via caching

Both personas MUST be served by the same binary. Feature activation is configuration-driven (ref: PM-01).

## 5. Goals

### 5.1 MUST Have (P0)

| ID | Feature | Description | Ref |
|----|---------|-------------|-----|
| F-001 | Cross-request cache | Cache conversation context by `conversation_id`, enabling reuse across requests to reduce upstream calls and latency | SA-01, SME-01 |
| F-002 | Request metrics | Capture TTFB, total latency, token counts, error rates, and cache hit/miss per request. Expose via Admin API | SA-02, PM-02 |
| F-003 | Unified error mapping | Map all Kiro-specific errors to Anthropic-compatible error responses. No raw upstream errors SHALL reach clients | SA-03 |
| F-004 | Converter enhancements | Integrate improved model mapping, tool schema normalization, and content block handling from dev-source | SA-04 |
| F-008 | CredentialIdentity trait | Shared trait providing domain-separated identity for detection subsystem (fingerprint) and cache subsystem (cache key). Foundation for F-001 integration | SA-08, C-003 |

### 5.2 SHOULD Have (P1)

| ID | Feature | Description | Ref |
|----|---------|-------------|-----|
| F-005 | Prompt presets | Named system prompt templates, selectable via request header or Admin API. Reduces repetition for common workflows | PM-03 |
| F-006 | PDF support | Extract text from PDF attachments using `lopdf`, convert to text content blocks for upstream submission | SME-03 |

### 5.3 SHOULD Have (P1, Internal)

| ID | Feature | Description | Ref |
|----|---------|-------------|-----|
| F-007 | Core module refactoring | Gradual refactoring of `provider.rs`, `stream.rs`, and `converter.rs` for maintainability. MUST NOT change external behavior | SA-07, TS-01 |

## 6. Non-Goals

The following items are explicitly out of scope for the Fusion initiative:

- **Deployment process changes**: Docker build, CI/CD pipelines, and release workflows remain as-is (ref: PM-04)
- **Multi-tenant SaaS-ification**: kiro.rs remains a self-hosted proxy. No tenant isolation, billing, or user management (ref: PM-05)
- **Prompt filter integration**: `prompt_filter.rs` from dev-source is deferred pending security review. Content filtering introduces liability and false-positive risk (ref: SME-04)
- **Breaking changes to Anthropic API compatibility**: All existing API contracts MUST be preserved. Clients MUST NOT require changes (ref: SA-05)
- **Frontend redesign**: The React 18 Admin UI receives additive features (metrics dashboard, preset management) but no visual overhaul
- **Multi-region deployment**: No distributed caching, no cross-region failover at the proxy layer
- **Distributed caching**: Cache is in-process only. External cache backends (Redis, etc.) are future work

## 7. Feature Specifications

### 7.1 F-001: Cross-Request Cache

**Purpose**: Reduce redundant upstream calls when clients send sequential messages in the same conversation.

**Mechanism**: Kiro's upstream API supports `conversation_id` for context reuse. The cache layer MUST:

- Store `conversation_id` keyed by a deterministic hash of conversation context (model + system prompt + message history prefix)
- Use `CredentialIdentity` (F-008) to scope cache entries per credential, preventing cross-credential cache pollution
- Implement TTL-based expiration (configurable, default 30 minutes)
- Expose cache hit/miss counters via F-002 metrics
- MUST NOT cache failed responses or error states

**Constraints**: Cache lookup MUST add less than 2ms latency (ref: TS-02). Cache MUST be disabled by default and opt-in via configuration.

### 7.2 F-002: Request Metrics

**Purpose**: Provide operators with full visibility into proxy behavior.

**Captured metrics** (per request):

- TTFB (time to first byte from upstream)
- Total request duration
- Input/output token counts (estimated)
- Cache hit/miss
- Credential used (anonymized)
- Error type (if any)
- Compression activation and ratio

**Exposure**: Admin API endpoint (`GET /admin/metrics`) returning JSON. In-memory ring buffer (configurable size, default 10,000 entries). No external metrics backend required.

### 7.3 F-003: Unified Error Mapping

**Purpose**: Ensure all client-facing errors conform to the Anthropic API error schema.

**Requirements**:

- MUST map Kiro-specific error codes to Anthropic `error` objects with `type`, `message` fields
- MUST preserve HTTP status code semantics (4xx for client errors, 5xx for server/upstream errors)
- MUST NOT expose internal Kiro error details, endpoint URLs, or credential information
- SHOULD include a `x-kiro-error-id` header for operator debugging (visible only with admin credentials)

### 7.4 F-004: Converter Enhancements

**Purpose**: Improve protocol translation fidelity between Anthropic and Kiro formats.

**Scope**: Integrate improvements from dev-source `converter.rs`:

- Enhanced model ID mapping for new Claude model variants
- Improved JSON Schema normalization (handle additional edge cases from MCP tools)
- Better content block type handling for multi-modal inputs
- MUST NOT regress existing conversion behavior (ref: SA-04)

### 7.5 F-005: Prompt Presets

**Purpose**: Allow operators to define reusable system prompt templates.

**Mechanism**:

- Presets stored in configuration file (JSON array)
- Selected via `x-prompt-preset` request header
- Preset content prepended to the system prompt in the request
- CRUD management via Admin API
- SHOULD support variable substitution (e.g., `{{user_name}}`)

### 7.6 F-006: PDF Support

**Purpose**: Enable PDF file attachments in messages.

**Mechanism**:

- Detect PDF content in base64-encoded `document` content blocks
- Extract text using `lopdf` crate
- Convert extracted text to `text` content blocks
- SHOULD preserve page boundaries as section markers
- MUST handle encrypted PDFs gracefully (return error, not crash)

**New dependency**: `lopdf` (ref: TS-02)

### 7.7 F-008: CredentialIdentity Trait

**Purpose**: Provide a shared abstraction for credential identity across subsystems.

**Design** (ref: SA-08, C-003):

```
trait CredentialIdentity {
    fn detection_identity(&self) -> DetectionId;  // For anti-detection subsystem
    fn cache_identity(&self) -> CacheId;          // For cross-request cache
}
```

Domain separation ensures that anti-detection fingerprinting and cache keying use independent identity derivations, preventing leakage between subsystems.

## 8. Architecture Constraints

These constraints are locked from the brainstorm phase and MUST NOT be violated:

| ID | Constraint | Rationale |
|----|-----------|-----------|
| C-001 | MUST NOT weaken compression, anti-detection, image processing, or Web Portal | These are competitive differentiators for Persona A. Regression is a blocking issue | 
| C-002 | MUST preserve credential management split (token_manager + cooldown + rate_limiter) | The current architecture handles complex failover scenarios. Merging subsystems introduces regression risk |
| C-003 | MUST implement domain-separated CredentialIdentity | Prevents anti-detection fingerprints from being used as cache keys (or vice versa), which would create correlation vectors |
| C-004 | SHOULD reference dev-source design but reimplement independently | Dev-source code assumes different module boundaries. Copy-paste integration creates maintenance debt |

## 9. Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Cache hit rate | >60% for repeat conversations | F-002 metrics, conversations with >2 turns |
| TTFB visibility | 100% of requests | F-002 metrics completeness audit |
| Error transparency | Zero raw Kiro errors reaching clients | Integration test suite + production error log audit |
| Added latency (metrics + cache) | <5ms P99 | Benchmark: proxy with/without F-001 + F-002 enabled |
| Compression activation rate | No regression vs v1.1.31 baseline | A/B comparison under equivalent workload |
| Existing test suite | 100% pass rate | `cargo test` green on every merge |

## 10. Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| Cache invalidation bugs cause stale responses | Medium | High | Conservative TTL defaults; cache disabled by default; explicit invalidation API |
| Metrics overhead degrades latency | Low | Medium | Ring buffer with fixed allocation; benchmark gating on merge (ref: TS-02) |
| Converter enhancement regresses existing translations | Medium | High | Comprehensive snapshot tests; parallel execution against dev-source for comparison |
| `lopdf` introduces security vulnerabilities (PDF parsing) | Low | Medium | Sandbox PDF processing; limit file size; fuzz testing with `proptest` |
| Anti-detection subsystem conflicts with cache identity | Low | High | Domain-separated CredentialIdentity (C-003) enforced at type level |

## 11. Scope Boundaries

### In Scope

- Features F-001 through F-008 as specified above
- Admin API extensions for metrics exposure and preset management
- Configuration schema additions for cache TTL, metrics buffer size, and preset definitions
- Integration and snapshot test suites for new features
- Documentation updates for configuration reference

### Out of Scope

- Frontend visual redesign (additive dashboard widgets only)
- Multi-region or distributed deployment topology
- External cache backends (Redis, Memcached)
- Distributed tracing (OpenTelemetry integration)
- Rate limiting changes (existing per-credential rate limiter is preserved, not extended)

## 12. Technology Stack

### Existing (Preserved)

| Component | Technology |
|-----------|-----------|
| Language | Rust 2024 edition |
| Web framework | Axum 0.8 |
| Async runtime | Tokio |
| Serialization | serde_json (with `preserve_order`) |
| Synchronization | parking_lot |
| Security | subtle (constant-time comparison) |
| Static embedding | rust-embed |
| Frontend | React 18 + TypeScript + Tailwind CSS |

### New Dependencies (Proposed)

| Crate | Purpose | Feature | Ref |
|-------|---------|---------|-----|
| `lopdf` | PDF text extraction | F-006 | TS-02 |
| `proptest` | Property-based testing (dev-dependency) | Test infrastructure | TS-02 |

New dependencies MUST be reviewed for license compatibility (MIT/Apache-2.0) and supply chain risk before adoption.

## 13. Phasing Guidance

This product brief does not prescribe a detailed implementation plan (see PRD for phasing). However, the following sequencing constraints apply:

1. **F-008 (CredentialIdentity) MUST precede F-001 (Cache)**: Cache keying depends on the identity trait
2. **F-003 (Error Mapping) SHOULD precede F-002 (Metrics)**: Metrics should capture mapped error types, not raw upstream codes
3. **F-007 (Refactoring) MUST be interleaved, not batched**: Refactoring in isolation creates merge conflicts. Each refactoring step MUST be paired with a feature integration step
4. **F-004 (Converter) MAY proceed independently**: Converter enhancements have minimal coupling to other features

## 14. Decision References

This product brief incorporates decisions from the brainstorm phase:

| ID | Decision | Status |
|----|----------|--------|
| SA-01 | Cross-request cache using conversation_id | Accepted |
| SA-02 | In-process metrics with Admin API exposure | Accepted |
| SA-03 | Unified error mapping layer | Accepted |
| SA-04 | Converter enhancements from dev-source | Accepted |
| SA-05 | No breaking changes to Anthropic API | Accepted |
| SA-07 | Gradual refactoring approach | Accepted |
| SA-08 | CredentialIdentity shared trait with domain separation | Accepted |
| SA-11 | Preserve CLI endpoint and Web Portal | Accepted |
| PM-01 | Configuration-driven feature activation | Accepted |
| PM-02 | Metrics via Admin API | Accepted |
| PM-03 | Prompt presets as SHOULD priority | Accepted |
| PM-04 | No deployment process changes | Accepted |
| PM-05 | No multi-tenant SaaS-ification | Accepted |
| SME-01 | Cache scoped per credential | Accepted |
| SME-03 | PDF support via lopdf | Accepted |
| SME-04 | Prompt filter deferred | Accepted |
| TS-01 | Interleaved refactoring | Accepted |
| TS-02 | Benchmark gating for new dependencies | Accepted |
| C-001 | Preserve competitive advantages | Locked |
| C-002 | Preserve credential management split | Locked |
| C-003 | Domain-separated CredentialIdentity | Locked |
| C-004 | Independent reimplementation over copy-paste | Locked |

---

*Document generated for BLP-kiro-fusion-2026-06-05. Next artifact: PRD with detailed phasing and acceptance criteria.*
