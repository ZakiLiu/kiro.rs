# Product Manager Analysis -- kiro.rs vs kiro-rs-dev-source Codebase Comparison

> Contract: guidance-specification.md S5 (decisions PM-01 through PM-05)
> Owns: Product positioning, feature prioritization, user segment definition, success metrics, roadmap shape, business value assessment
> Does not own: Technical architecture (SA), domain implementation patterns (SME), test strategy (TS), UI design

## 1. Role Mandate

This analysis defines the product strategy for merging capabilities from kiro-rs-dev-source into the current kiro.rs project. The product manager decides what gets built, in what order, and for whom -- not how it gets built. The central tension is PM-03: balancing the current "Swiss Army Knife" positioning (scale, evasion, compression) with dev-source's "Production Platform" strengths (observability, admin control, caching). The prioritization framework (PM-04) sequences features into three waves: P0 (cache + metrics) for immediate cost and visibility wins, P1 (error mapping + converter) for reliability and compatibility, P2 (PDF + prompt presets) for capability breadth. All decisions preserve existing strengths (PM-02) while closing the four most impactful gaps identified in dev-source. The user context confirms that all four dev-source features MUST be adopted, and all four current advantages MUST NOT be weakened. This analysis provides the business rationale, user segmentation, success metrics, and prioritization rationale that guide technical implementation across other roles.

## 2. Decision Digest

### Decisions
| ID | Feature | Stance | Constraints (RFC 2119) |
|----|---------|--------|------------------------|
| PM-01 | Cross-cutting | Adopt all four dev-source features: cache, metrics, error mapping, PDF | MUST implement all four; partial adoption is insufficient |
| PM-02 | Cross-cutting | Preserve all four current advantages: compression, anti-detection, image processing, web portal | MUST NOT weaken any existing capability during feature adoption |
| PM-03 | Cross-cutting | Balance Swiss Army Knife and Production Platform positioning | SHOULD serve both user segments through feature-group toggling |
| PM-04 | Cross-cutting | Three-wave prioritization: P0 then P1 then P2 | MUST sequence as P0: F-001+F-002+F-008, P1: F-003+F-004, P2: F-005+F-006 |
| PM-05 | F-005 | Adopt prompt presets, defer prompt filter to independent evaluation | SHOULD ship presets; MUST NOT ship filter without separate risk assessment |
| PM-06 | F-001 | Cache is the highest-ROI feature: reduces upstream token cost for repeat conversations | MUST integrate with CredentialIdentity (F-008), MUST expose hit rate via metrics (F-002) |
| PM-07 | F-002 | Metrics close the most critical operational gap: zero visibility today | MUST track latency/TTFB/error-rate/credential-distribution |
| PM-08 | F-003 | Error mapping directly improves downstream client experience | MUST produce Anthropic-formatted errors with Retry-After headers |
| PM-09 | F-007 | Module refactoring is a force multiplier, not a user-facing feature | SHOULD proceed incrementally alongside feature work, MUST NOT block P0 |
| PM-10 | F-006 | PDF support completes the multimodal content pipeline alongside image processing | MUST fail gracefully on malformed PDFs; MAY defer to P2 |

### Interfaces

> **Cross-Role Synergy (S-003)**: Aligns with SA Configuration section — per-feature enabled:bool flags in CrossRequestCacheConfig/MetricsConfig directly implement the toggle strategy for dual-persona serving.

| Name | Contract | Consumers |
|------|----------|-----------|
| Priority Framework (P0/P1/P2) | Feature sequencing with dependency ordering | SA (implementation planning), TS (test sequencing), SME (domain priorities) |
| User Segments | Power-user (stealth-focused) vs Operator (platform-focused) | All roles (feature scoping decisions) |
| Success Metrics | Cache hit rate, TTFB reduction, error classification accuracy | SA (architecture validation), TS (test targets) |
| Feature Toggle Strategy | Independent feature groups configurable at runtime | SA (config schema design), SME (default configurations) |

### Cross-Cutting Positions
| Topic | Stance |
|-------|--------|
| Personas | Two primary segments: Power User (stealth/scale) and Platform Operator (visibility/control) |
| Success Metrics | Cache hit rate >60% for repeat conversations; TTFB visibility for 100% of requests; zero raw-Kiro errors reaching clients |
| Roadmap Shape | Three waves over ~3 releases: P0 (foundational), P1 (reliability), P2 (breadth) |
| Prioritization Rationale | Cost reduction (cache) and visibility (metrics) first because they have the highest impact-to-effort ratio and no reverse dependencies |
| Feature Toggle | Stealth features and platform features independently toggleable to serve both segments from one binary |
| Prompt Filter | Deferred indefinitely; legal/TOS/detection risks outweigh user value |

### Findings Summary
| Slug | Title | Impact |
|------|-------|--------|
| dual-positioning-tension | Dual Positioning Tension Between Swiss Army Knife and Production Platform | HIGH |
| prompt-filter-risk | Prompt Filter Carries Non-Trivial Legal and Reputational Risk | MEDIUM |

## 3. Cross-Cutting Foundations

### Personas

**Power User (Stealth-Focused)**: Operates the proxy for high-volume, cost-sensitive workloads where upstream detection avoidance is critical. Values compression pipeline (handles 5MB limit), anti-detection (fingerprint diversity), image processing (multimodal support), and credential failover (uptime). Typical deployment: personal or small-team use, single operator, minimal admin overhead. This user MUST NOT lose existing capabilities during the merge.

**Platform Operator (Visibility-Focused)**: Operates the proxy as infrastructure for a team or organization. Values metrics (operational visibility), error mapping (client experience), prompt presets (behavior customization), and admin UI (credential management). Typical deployment: shared service, multiple consumers, requires observability and access control. This user drives demand for all four dev-source features.

Both personas share the core proxy value: Anthropic API compatibility over Kiro infrastructure with credential failover. The feature-group toggle strategy (see findings-dual-positioning-tension.md) SHOULD allow a single binary to serve both without forcing irrelevant configuration.

### Success Metrics

| Metric | Target | Measurement | Feature |
|--------|--------|-------------|---------|
| Cache hit rate | >60% for repeat conversations | F-002 metrics / Admin API | F-001 |
| TTFB visibility | 100% of requests tracked | Ring buffer completeness | F-002 |
| Error classification | Zero raw Kiro errors reaching clients | Error type distribution in metrics | F-003 |
| PDF extraction success rate | >95% for standard PDFs | Success/failure counter in metrics | F-006 |
| Compression preservation | No regression in compression activation rate | Before/after comparison via metrics | PM-02 |
| Request latency overhead | <5ms added by metrics + cache lookup | p99 latency comparison | F-001, F-002 |

### Roadmap Shape

**P0 -- Foundation (Release N)**: F-008 (CredentialIdentity trait) + F-001 (cross-request cache) + F-002 (request metrics). Rationale: cache delivers the highest ROI (direct cost reduction via token reuse), metrics close the most critical operational gap, and CredentialIdentity is a hard prerequisite for cache. These three features have no external dependencies and can be developed in parallel after F-008 is defined.

**P1 -- Reliability (Release N+1)**: F-003 (error mapping) + F-004 (converter enhancement). Rationale: error mapping improves client experience and depends on metrics for error-type counters. Converter enhancement depends on CredentialIdentity for conversation_id injection. F-007 (module refactoring) SHOULD begin opportunistically during P1, starting with converter.rs split to accommodate F-004 changes.

**P2 -- Breadth (Release N+2)**: F-005 (prompt presets) + F-006 (PDF support). Rationale: these features expand capability breadth but are not blocking for core proxy reliability. They can be developed independently and shipped when ready.

### Prioritization Rationale

The P0/P1/P2 sequencing follows three principles:

1. **Dependency ordering**: F-008 enables F-001 and F-004; F-002 enables monitoring of F-001 and F-003. Dependencies flow forward, never backward.
2. **Impact-to-effort ratio**: Cache (F-001) and metrics (F-002) have the highest ratio. Cache directly reduces upstream costs for every repeat conversation. Metrics provide the visibility foundation that all subsequent features need for validation.
3. **Risk isolation**: Each wave can be shipped and validated independently. P0 features do not depend on P1 completion; P1 features do not depend on P2. If development stalls at any wave, prior waves are fully functional.

Module refactoring (F-007) is cross-version because it delivers no direct user value and SHOULD be driven by implementation needs rather than a fixed schedule.

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
| [findings-dual-positioning-tension.md](findings-dual-positioning-tension.md) | finding | -- | Description, Affected Features, Recommendation |
| [findings-prompt-filter-risk.md](findings-prompt-filter-risk.md) | finding | -- | Description, Affected Features, Recommendation |

## 5. Outstanding TODOs

- Validate cache hit rate target (>60%) against real-world conversation patterns from current usage data
- Quantify upstream cost savings from cross-request cache to build business case for P0 prioritization
- Survey current users to validate Power User vs Platform Operator segment split
- Define feature toggle schema (which features are grouped, default on/off states) -- coordinate with SA
- Conduct independent evaluation of prompt filter (F-005 dependency): legal review, TOS analysis, detection risk assessment
- Determine if the combined feature set pushes binary size beyond acceptable limits for single-binary deployment
- Establish baseline TTFB measurements before P0 implementation to enable before/after comparison
- Evaluate whether dev-source calendar versioning (2026.3.1) or current semver (1.1.31) is the right scheme post-merge