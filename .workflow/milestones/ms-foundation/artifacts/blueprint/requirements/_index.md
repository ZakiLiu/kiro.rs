---
document: requirements-prd
session_id: BLP-kiro-fusion-2026-06-05
version: "1.0"
status: draft
---

# Requirements / PRD — kiro.rs Fusion Blueprint

> 将 kiro-rs-dev-source 的 cache、metrics、error mapping、PDF 能力融入 kiro.rs，同时保持 compression、anti-detection、image processing、Web Portal 不受削弱。

## 1. MoSCoW Priority Matrix

| Priority | REQ ID | Feature | Wave | Rationale |
|----------|--------|---------|------|-----------|
| **Must** | REQ-001 | Cross-Request Cache | P0 | 最高 ROI — 复用 conversation_id 直接降低上游 token 成本 |
| **Must** | REQ-002 | Request Metrics | P0 | 填补零可观测性的核心缺口，后续 feature 验证基础 |
| **Must** | REQ-003 | Error Mapping | P1 | 消除分散在 4+ 文件中的内联错误翻译，统一客户端错误体验 |
| **Must** | REQ-004 | Converter Enhancement | P1 | Tool name shortening 解除上游字符限制，conversation_id 注入启用缓存 |
| **Must** | REQ-008 | Shared Identity (CredentialIdentity) | P0 | F-001 和 F-004 的硬依赖；域分离阻止上游关联攻击 |
| **Should** | REQ-005 | Prompt Presets | P2 | 运行时行为定制，Platform Operator 用户需求 |
| **Should** | REQ-006 | PDF Support | P2 | 补全多模态内容管道（与现有图片处理对齐） |
| **Should** | REQ-007 | Module Refactor | Cross-version | 力量倍增器，按 feature 需要渐进拆分 |

## 2. Traceability Matrix — REQs to Brainstorm Decisions

| REQ | SA Decisions | PM Decisions | SME Decisions | TS Decisions |
|-----|-------------|-------------|---------------|-------------|
| REQ-001 | SA-02 (cache key design) | PM-06 (highest ROI) | SME-01, SME-05 (domain-separated key, layer on CacheTracker) | TS-03 (LRU/TTL deterministic tests) |
| REQ-002 | SA-07 (ring buffer metrics) | PM-07 (critical gap) | SME-07 (five domain signals) | TS-04 (overflow + concurrency) |
| REQ-003 | SA-03 (dedicated error_map) | PM-08 (client experience) | SME-06 (five Kiro error categories) | TS-05 (all status codes) |
| REQ-004 | SA-08 (shortening + injection) | PM-04 (P1 wave) | SME-03 (domain-justified) | TS-06 (round-trip fidelity) |
| REQ-005 | SA-09 (configurable presets) | PM-05 (defer filter) | SME-08 (presets valuable, filter deferred) | TS-07 (malformed resilience) |
| REQ-006 | SA-10 (lopdf integration) | PM-10 (multimodal pipeline) | SME-09 (feature-gated) | TS-08 (real fixtures) |
| REQ-007 | SA-01 (incremental decomposition) | PM-09 (force multiplier) | SME-02 (preserve current separation) | TS-09 (zero regression) |
| REQ-008 | SA-04 (CredentialIdentity trait) | PM-04 (P0 prerequisite) | SME-01, SME-04 (domain separation, competitive moat) | TS-10 (cross-module consistency) |

## 3. Constraints (Locked)

| ID | Area | Constraint | Source |
|----|------|-----------|--------|
| C-001 | Anti-Detection | MUST NOT weaken compression, anti-detection, image processing, or Web Portal | PM-02, SME-04 |
| C-002 | Architecture | MUST preserve credential management split (token_manager + cooldown + rate_limiter) | SA-05, SME-02 |
| C-003 | Architecture | MUST implement CredentialIdentity with domain-separated identities | SA-04, SME-01 |
| C-004 | Implementation | SHOULD reimplement from dev-source design, not transplant code | TS-02 |

## 4. Non-Functional Requirements Summary

| NFR ID | Category | Title | Target | REQ Dependencies |
|--------|----------|-------|--------|-----------------|
| NFR-PERF-001 | Performance | Latency Overhead | <5ms from cache+metrics; record() <500ns | REQ-001, REQ-002 |
| NFR-SEC-001 | Security | Anti-Detection Preservation | Fingerprint diversity preserved; cache key uncorrelated with detection identity | REQ-001, REQ-008 |
| NFR-REL-001 | Reliability | Failover Continuity | Credential failover unchanged; error classification drives retry; cache invalidation on cooldown | REQ-003, REQ-001 |

## 5. Personas

| Persona | Focus | Key Features | Constraint |
|---------|-------|-------------|-----------|
| **Power User** | Stealth / Scale | Compression, anti-detection, credential failover, cache | MUST NOT lose existing capabilities |
| **Platform Operator** | Visibility / Control | Metrics, error mapping, prompt presets, Admin UI | Drives demand for dev-source features |

## 6. Wave Roadmap

```
P0 (Foundation)  → REQ-008 + REQ-001 + REQ-002
P1 (Reliability)  → REQ-003 + REQ-004 + REQ-007 (opportunistic)
P2 (Breadth)      → REQ-005 + REQ-006
```

Dependencies flow forward, never backward. Each wave is independently shippable.

## 7. File Index

| File | REQ ID | Priority |
|------|--------|----------|
| [REQ-001-cross-request-cache.md](REQ-001-cross-request-cache.md) | REQ-001 | Must |
| [REQ-002-request-metrics.md](REQ-002-request-metrics.md) | REQ-002 | Must |
| [REQ-003-error-mapping.md](REQ-003-error-mapping.md) | REQ-003 | Must |
| [REQ-004-converter-enhance.md](REQ-004-converter-enhance.md) | REQ-004 | Must |
| [REQ-005-prompt-presets.md](REQ-005-prompt-presets.md) | REQ-005 | Should |
| [REQ-006-pdf-support.md](REQ-006-pdf-support.md) | REQ-006 | Should |
| [REQ-007-module-refactor.md](REQ-007-module-refactor.md) | REQ-007 | Should |
| [REQ-008-shared-identity.md](REQ-008-shared-identity.md) | REQ-008 | Must |
| [NFR-PERF-001-latency.md](NFR-PERF-001-latency.md) | NFR-PERF-001 | Must |
| [NFR-SEC-001-anti-detection.md](NFR-SEC-001-anti-detection.md) | NFR-SEC-001 | Must |
| [NFR-REL-001-failover.md](NFR-REL-001-failover.md) | NFR-REL-001 | Must |
