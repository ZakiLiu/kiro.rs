---
document: blueprint-summary
session_id: BLP-kiro-fusion-2026-06-05
version: "1.0"
status: complete
---

# Blueprint Summary — kiro.rs Fusion

## One-Line

Fuse cross-request caching, metrics, error mapping, and converter enhancements from kiro-rs-dev-source into kiro.rs while preserving compression, anti-detection, image processing, and Web Portal capabilities.

## Scope

**8 features** decomposed into **4 Epics** with **15 stories** (~40 story points):

| Wave | Features | Priority | Epic |
|------|----------|----------|------|
| P0 Foundation | CredentialIdentity trait + Cross-request cache + Metrics | Must | EPIC-001 (MVP) |
| P1 Reliability | Error mapping + Converter enhancement | Must | EPIC-002 |
| P2 Capability | Prompt presets + PDF support | Should | EPIC-003 |
| Cross-version | Module refactoring (converter/stream/token_manager) | Should | EPIC-004 |

## Key Decisions

1. **CredentialIdentity trait** (ADR-001): Three methods with domain-separated SHA-256 — detection and cache identities are cryptographically independent
2. **Cache topology** (ADR-002): Cross-request cache layers ON TOP of existing CacheTracker, does not replace it
3. **Error mapper** (ADR-003): Dual-function pattern (classify + to_anthropic_response) with RequestContext carrying compression state and upstream headers
4. **Metrics** (ADR-004): In-memory ring buffer (10K entries), no external dependencies, Admin API aggregation

## Constraints (Locked)

- MUST NOT weaken anti-detection, compression, image processing, or Web Portal
- MUST preserve credential management split architecture
- MUST preserve CLI endpoint (kiro/endpoint/cli.rs)
- MUST implement domain-separated CredentialIdentity
- SHOULD reimplement from dev-source design, not transplant code

## Success Metrics

| Metric | Target |
|--------|--------|
| Cache hit rate | >60% for repeat conversations |
| TTFB visibility | 100% of requests tracked |
| Error classification | Zero raw Kiro errors reaching clients |
| Latency overhead | <5ms from cache + metrics |
| Compression preservation | No regression |

## Readiness

**Score: 93% — PASS**

25 files across 6 phases. Full traceability from brainstorm decisions through requirements, architecture, and epics. 215 RFC 2119 keyword instances across 22 files.

## Next Steps

- `/maestro-roadmap --from blueprint:BLP-kiro-fusion-2026-06-05` — Generate execution roadmap
- `/maestro-analyze 1 --from blueprint:BLP-kiro-fusion-2026-06-05` — Deep-analyze EPIC-001
- `/maestro-plan 1` — Plan first phase directly
