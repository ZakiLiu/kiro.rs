# Test Strategist Analysis — kiro.rs vs kiro-rs-dev-source Codebase Comparison

> Contract: guidance-specification.md §7 (decisions TS-01, TS-02)
> Owns: test strategy, quality gates, coverage targets, risk-based test prioritization, test tooling recommendations
> Does not own: architecture decisions (SA), feature prioritization (PM), domain design patterns (SME)

## 1. Role Mandate

The test strategist defines the testing strategy for integrating dev-source features into kiro.rs while preserving existing quality. The current codebase already contains 378 unit tests across 33 files — a stronger baseline than initially assumed. The primary challenge is not "bootstrap testing from zero" but rather "extend coverage to newly introduced modules (cache, metrics, error mapping, converter enhancements) while maintaining the existing test contract." This role decides which test layers apply to each feature, what coverage thresholds to enforce, how to handle untestable external dependencies (Kiro API, binary protocol), and where property-based testing delivers the highest ROI. Architecture and implementation choices are deferred to SA and SME roles respectively.

## 2. Decision Digest

### Decisions
| ID | Feature | Stance | Constraints (RFC 2119) |
|----|---------|--------|------------------------|
| TS-01 | cross-cutting | Critical paths MUST have test coverage before merging any new feature | MUST cover protocol conversion, cache logic, error mapping |
| TS-02 | cross-cutting | New features SHOULD be designed test-first, referencing dev-source patterns but implementing independently | SHOULD use dev-source as design reference only |
| TS-03 | F-001 cross-request-cache | Cache module MUST have deterministic tests for LRU eviction, TTL expiry, and fingerprint-based key resolution | MUST achieve ≥ 90% branch coverage on cache core |
| TS-04 | F-002 request-metrics | Metrics ring buffer MUST be tested for overflow, windowed aggregation, and concurrent access | MUST validate thread safety under contention |
| TS-05 | F-003 error-mapping | Error mapping MUST cover all upstream status codes (400, 401, 402, 403, 429, 500, 502, 503) with Retry-After injection | MUST NOT leave unmapped error paths |
| TS-06 | F-004 converter-enhance | Tool name shortening and forced conversation_id MUST have round-trip fidelity tests | MUST verify shortening reversibility |
| TS-07 | F-005 prompt-presets | Preset loading SHOULD be tested for malformed YAML/JSON resilience | SHOULD cover at least 3 preset variants |
| TS-08 | F-006 pdf-support | PDF extraction MUST be tested with real fixtures (empty, encrypted, CJK, large) | MUST handle malformed PDF gracefully |
| TS-09 | F-007 module-refactor | Module refactoring MUST NOT break existing 378 tests — zero regression is the gate | MUST pass `cargo test` before and after each split |
| TS-10 | F-008 shared-identity | CredentialIdentity trait MUST be tested for cross-module consistency (same input produces same fingerprint) | MUST verify isolation between cache key and affinity fingerprint derivations |

### Interfaces

> **Cross-Role Gap (G-002)**: SA has not acknowledged coverage gate integration — coordinate with SA on F-007 to ensure module splits preserve TS coverage thresholds.

| Name | Contract | Consumers |
|------|----------|-----------|
| Test Fixture Library | Shared JSON/binary fixtures under `tests/fixtures/` | All feature test modules |
| Coverage Gate | `cargo llvm-cov` threshold ≥ 80% on new modules | CI pipeline (future), SA |
| Property Test Harness | `proptest` crate for compression and cache invariants | F-001, F-003, compressor |
| Integration Test Boundary | `tests/integration/` directory for cross-module flows | F-001 × F-008, F-003 × compression |

### Cross-Cutting Positions
| Topic | Stance |
|-------|--------|
| Test Layers | Three-layer pyramid: unit (80%) / integration (15%) / manual-exploratory (5%); no UI E2E for admin panel in first phase |
| Coverage Targets | New modules MUST achieve ≥ 80% line coverage; existing modules SHOULD maintain current coverage |
| Risk-Based Prioritization | P0 features (cache, metrics) get test-first development; P2 features (PDF, presets) get test-after |
| Tooling | `cargo test` + `proptest` + `cargo llvm-cov`; no external test orchestration needed |

### Findings Summary
| Slug | Title | Impact |
|------|-------|--------|
| existing-coverage-baseline | Existing test suite is substantial (378 tests, 33 files) | HIGH — reframes strategy from greenfield to incremental extension |
| compression-property-gap | Compression pipeline lacks property-based testing despite being ideal candidate | MEDIUM — proptest can catch edge cases in multi-stage pipeline |
| integration-test-absence | No integration tests exist; all tests are unit-level | MEDIUM — cross-module flows (cache × fingerprint, error map × compression) are untested |

## 3. Cross-Cutting Foundations

### Test Layers

The project MUST adopt a three-layer test pyramid:

1. **Unit tests** (target: 80% of test effort) — Pure function testing with no I/O. Each new module (cache, metrics, error_map, converter enhancements) MUST have co-located `#[cfg(test)]` modules following the established pattern. The existing codebase already demonstrates this consistently (converter.rs: 56 tests, stream.rs: 37 tests, compressor.rs: 33 tests).

2. **Integration tests** (target: 15% of test effort) — Cross-module flows in `tests/` directory. Priority scenarios: (a) cache lookup with CredentialIdentity-derived keys, (b) error mapping after compression-induced upstream failure, (c) converter to stream end-to-end with tool name shortening.

3. **Manual/exploratory testing** (target: 5%) — Limited to anti-detection behavioral validation and admin UI interaction, which are inherently resistant to automated testing.

### Coverage Targets

Coverage MUST be tracked per-module using `cargo llvm-cov`. Thresholds:

- New modules (F-001 through F-008): MUST achieve ≥ 80% line coverage before merge.
- Existing modules with tests (converter, stream, compressor): MUST NOT regress below current coverage after refactoring (see TS-09).
- Modules without tests (affinity.rs, web_portal.rs): SHOULD add basic smoke tests during module refactor (F-007) but MAY defer comprehensive coverage.

### Risk-Based Prioritization

> **Cross-Role Synergy (S-002)**: SME Pitfall Taxonomy provides domain-severity ratings for integration test prioritization — cache-detection correlation (HIGH), compression-induced 400 (MEDIUM), tool name collision (LOW).

Test development MUST follow risk ranking:

| Risk Tier | Modules | Rationale |
|-----------|---------|-----------|
| Critical | cache core (F-001), error_map (F-003), converter (F-004) | Data correctness and API compatibility — bugs cause silent corruption or client-visible errors |
| High | metrics (F-002), CredentialIdentity (F-008) | Operational visibility and cross-module contract — bugs cause monitoring blind spots or fingerprint leaks |
| Medium | PDF extraction (F-006), prompt presets (F-005) | Additive features — bugs cause feature degradation, not system failure |
| Low | module refactor (F-007) | Structural change — existing 378 tests serve as regression gate |

### Tooling

The project MUST use the following test tooling stack:

- **`cargo test`** — Primary test runner. Already in use via `make check`.
- **`proptest`** — Property-based testing for compression pipeline, cache eviction invariants, and error mapping completeness. SHOULD be added as a dev-dependency.
- **`cargo llvm-cov`** — Coverage measurement. SHOULD be integrated into `make check` or a separate `make coverage` target.
- **`tokio::test`** — Async test runtime for integration tests involving concurrent cache access and metrics ring buffer.
- **Test fixtures** — Shared fixture directory (`tests/fixtures/`) for binary Event Stream frames, malformed JSON, sample PDF files, and Anthropic/Kiro request/response pairs.

The project SHOULD NOT introduce external test orchestration tools (e.g., nextest) in this phase. The built-in `cargo test` harness is sufficient for the current scale.

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
| [findings-existing-coverage-baseline.md](findings-existing-coverage-baseline.md) | finding | — | Description, Affected Features, Recommendation |
| [findings-compression-property-gap.md](findings-compression-property-gap.md) | finding | — | Description, Affected Features, Recommendation |
| [findings-integration-test-absence.md](findings-integration-test-absence.md) | finding | — | Description, Affected Features, Recommendation |

## 5. Outstanding TODOs

- Run `cargo llvm-cov` to establish precise baseline coverage numbers for each existing module.
- Audit dev-source test fixtures (if available) for reusable binary Event Stream samples.
- Evaluate `proptest` crate compatibility with Rust 2024 edition and current dependency tree.
- Define exact `Retry-After` injection rules for error mapping by studying upstream Kiro error response formats.
- Determine whether `tokio::test` or `tokio::runtime::Builder` is preferred for async integration tests given existing patterns.
- Create `tests/fixtures/` directory structure and seed with representative request/response pairs from production logs (sanitized).
