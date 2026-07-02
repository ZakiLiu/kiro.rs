---
related:
  - "spec:project:architecture-constraints"
---

# Roadmap: kiro-rs Fusion

## Overview

将 kiro-rs-dev-source 的核心能力（跨请求缓存、可观测性、统一错误映射、转换层增强、系统提示预设、PDF 支持）融入当前 kiro.rs 项目，同时完整保留现有竞争优势（压缩管道、Anti-Detection、图片处理、Web Portal、CLI 端点）。采用三阶段渐进式交付：Foundation (MVP) → Reliability → Capability，每个里程碑独立可交付、可验证。

## Milestones

### Milestone 1: Foundation — MVP (v1.2.0)
**Target**: 建立 CredentialIdentity 共享 trait、跨请求前缀缓存、请求指标可观测性三大基础能力
**Status**: planned

#### Phases

- [x] **Phase 1: Foundation Infrastructure** — CredentialIdentity trait + CrossRequestCache + MetricsCollector ✅ (2026-06-05)

#### Phase Details

##### Phase 1: Foundation Infrastructure
**Goal**: 实现三大 P0 功能，为后续所有 feature 提供身份抽象、缓存基础和可观测性基座
**Depends on**: Nothing (first phase)
**Requirements**: REQ-008 (CredentialIdentity), REQ-001 (Cross-Request Cache), REQ-002 (Request Metrics)
**Epic**: EPIC-001 (5 stories, ~13 story points)
**Success Criteria** (what must be TRUE):
  1. CredentialIdentity trait 在 `src/kiro/identity.rs` 中实现，detection_identity() 和 cache_identity() 产出互不可推导的域分离标识
  2. CrossRequestCache 在首次请求时记录 (cache_key, conversation_id)，后续相同前缀请求命中缓存并注入 forced_conversation_id，cache hit rate 在重复对话场景下 >60%
  3. MetricsCollector 通过 ring buffer 记录每请求延迟、TTFB、错误分类、凭据分布，Admin API 暴露窗口聚合查询
  4. `cargo test` 全绿，新模块单测覆盖 ≥80%
  5. 现有 anti-detection 和 compression 管道无回归（constraint C-001 verified）

---

### Milestone 2: Reliability (v1.3.0)
**Target**: 统一错误映射 + 转换层增强，提升下游客户端体验和协议兼容性
**Status**: planned

#### Phases

- [x] **Phase 1: Error & Converter Hardening** — ErrorMapper + RequestContext (Tool Name Shortening already existed) ✅ (2026-06-05)

#### Phase Details

##### Phase 1: Error & Converter Hardening
**Goal**: 消除分散的内联错误处理，引入统一 ErrorMapper；增强 converter 处理超长工具名和会话复用
**Depends on**: Milestone 1 Phase 1 (CredentialIdentity trait, CrossRequestCache)
**Requirements**: REQ-003 (Error Mapping), REQ-004 (Converter Enhancement)
**Epic**: EPIC-002 (4 stories, ~10 story points)
**Success Criteria** (what must be TRUE):
  1. ErrorMapper 的 classify() + to_anthropic_response() 覆盖所有上游状态码（400/401/402/403/429/500/502/503），零原始 Kiro 错误泄露到客户端
  2. 429/503 响应自动注入 Retry-After 头，下游客户端（Claude Code、Cursor 等）正确执行重试退避
  3. 超长工具名（>50 chars）自动缩写为 SHA-256 截断 8 hex 映射，响应中正确恢复原始名称
  4. 压缩引发的 400 错误通过 RequestContext.was_compressed 被 ErrorMapper 正确识别并记录诊断日志
  5. Metrics 中新增 error_class_count 统计维度

---

### Milestone 3: Capability (v1.4.0)
**Target**: 扩展能力面——系统提示预设 + PDF 文档支持 + 核心模块渐进拆分
**Status**: planned

#### Phases

- [x] **Phase 1: Features & Refactoring** — Prompt Presets + PDF Support + Module Decomposition ✅ (2026-06-06)

#### Phase Details

##### Phase 1: Features & Refactoring
**Goal**: 补全多模态内容管道（PDF）、运行时行为定制（presets）、代码结构优化（模块拆分）
**Depends on**: Milestone 2 Phase 1 (ErrorMapper 已就绪，converter 增强完成)
**Requirements**: REQ-005 (Prompt Presets), REQ-006 (PDF Support), REQ-007 (Module Refactor)
**Epic**: EPIC-003 (3 stories, ~8 pts) + EPIC-004 (3 stories, ~9 pts)
**Success Criteria** (what must be TRUE):
  1. 系统提示预设可通过 config.json 配置，Admin API 支持运行时切换预设（不含 prompt filter）
  2. Document 块中 base64 编码的 PDF 自动提取文本，支持 ≤32MB / ≤200K chars，提取失败优雅降级为占位符
  3. converter.rs 拆分为 6 个子模块、stream.rs 拆分为 5 个子模块，每次拆分独立 commit 且 `cargo test` 保持全绿
  4. 模块拆分后新子模块行覆盖率 ≥80%（TS Coverage Gate）
  5. lopdf 依赖通过 feature gate 控制，可在编译时排除

---

### Milestone 4: External IdP Support (v1.5.0)
**Target**: 完整引入 external_idp（Enterprise SSO / Azure AD）凭据支持，覆盖 JSON 导入、token 刷新、浏览器 SSO 登录、CLI 加载、Admin UI 导入
**Status**: planned
**Source**: brainstorm:brainstorm-external-idp-porting (2026-07-02)

#### Phases

- [ ] **Phase 1: Full External IdP Integration** — 凭据模型扩展 + SSRF 防护 + token 刷新 + 导入路径 + CLI 加载 + Admin UI + 浏览器 SSO

#### Phase Details

##### Phase 1: Full External IdP Integration
**Goal**: kiro.rs 完整支持 external_idp 凭据认证，IDE/CLI 等 endpoint 请求可通过 Azure AD 企业 SSO 账号成功
**Depends on**: Nothing (independent of Milestones 1-3; reuses existing token_manager/auth infrastructure)
**Requirements**: F-001 (凭据模型), F-002 (token 刷新), F-003 (SSRF 防护), F-004 (导入路径), F-005 (Admin UI), F-006 (CLI SQLite), F-007 (浏览器 SSO)
**Reference**: Kiro-Go 075df7a..a2b2c4d (14 commits, ~2250 LOC)

**Wave DAG** (task ordering within phase):
```
Wave 1 (foundations, parallel):
  F-001 credential-model — KiroCredentials 扩展 + serde alias
  F-003 ssrf-validation   — allow-list + validate_external_idp_endpoint()

Wave 2 (core, depends on Wave 1):
  F-002 token-refresh  — refresh routing + OIDC token endpoint POST
  F-004 import-path    — Admin API import + normalize + trust-on-import
  F-006 cli-sqlite     — CLI SQLite external_idp token key

Wave 3 (extended, depends on Wave 1):
  F-005 admin-ui       — frontend import field mapping
  F-007 browser-sso    — OIDC discovery + PKCE + callback server
```

**Success Criteria** (what must be TRUE):
  1. `auth_method: "external_idp"` 凭据通过 credentials.json 或 Admin API JSON 导入后可成功发起 API 请求
  2. token 自动刷新（OIDC refresh_token grant）正常工作，过期 token 后台自动续期
  3. trust-on-import：JWT exp 未过期的凭据可直接导入，无需 Microsoft egress
  4. SSRF 防护：导入和 refresh 双边界验证 tokenEndpoint（HTTPS + 禁 IP + allow-list）
  5. CLI SQLite 中的 external_idp 凭据可被自动加载
  6. Admin UI 可导入 external_idp 凭据（支持 Kiro IDE camelCase 和 helper snake_case 双格式）
  7. 浏览器 SSO 登录流程可通过 OIDC/Azure AD 完成认证
  8. 现有 social/idc 凭据行为零回归，`cargo test` 全绿

**Cross-Role Resolutions** (from brainstorm):
  - C-001: SSRF 验证函数参数化 allow-list（生产常量，测试自定义）
  - C-002: TokenJsonItem 逐字段加 serde alias（不改 rename_all）
  - G-001: is_social_login() 增加 external_idp 前置判定
  - G-002: expires_at 加 `#[serde(alias = "expired")]`
  - G-003: ExternalIdpRefreshResponse 不用 rename_all camelCase
  - S-001: 浏览器 SSO 复用 social.rs callback/PKCE 基础设施
  - S-002: 复用 DisableReason 枚举和冷却机制

**Blocked Items**:
  - DA-03: CLI SQLite external_idp token key 实际命名（需 dump auth_kv 表确认）
  - TS-05: examples/ 凭据已过期，手动测试前需重新登录 Kiro IDE/CLI

---

## Scope Decisions

- **In scope**: Milestone 1-3 原有 feature（已完成）；Milestone 4: external_idp 全量支持（F-001~F-007，含浏览器 SSO + trust-on-import + CLI SQLite）
- **Deferred**: 非 Microsoft IdP 支持（allow-list 扩展）；Prompt filter；token_manager 内部拆分
- **Out of scope**: SAML 协议支持；多租户 IdP 管理界面；前端 UI 重新设计；多区域部署；分布式缓存

## Roadmap Decisions

| # | Decision | Choice | Source |
|---|----------|--------|--------|
| 1 | Decomposition strategy | Progressive (3 milestones) | user |
| 2 | MVP scope | EPIC-001: CredentialIdentity + Cache + Metrics | blueprint PM-04 |
| 3 | Milestone boundaries | P0/P1/P2 from brainstorm priority waves | brainstorm + blueprint |
| 4 | Phase count per milestone | 1 phase each (minimum-phase principle) | roadmap-common |
| 5 | EPIC-004 placement | Milestone 3 (alongside EPIC-003) | user (progressive) |
| 6 | Prompt filter | Deferred — not in any milestone | brainstorm PM-05, blueprint W-001 |
| 7 | CLI endpoint | MUST preserve in all milestones (SA-11) | user explicit constraint |
| 8 | Milestone 4 scope | 全量移植 external_idp（含浏览器 SSO） | user |
| 9 | Milestone 4 分解 | 单 Phase + Wave DAG（3 waves, 7 features） | user + minimum-phase |
| 10 | trust-on-import | 包含在 F-004（JWT exp 解析跳过 refresh） | user |
| 11 | CLI SQLite | 包含在 F-006（external_idp token key） | user |
| 12 | 参考实现 | Kiro-Go 075df7a..a2b2c4d | code |

## Progress

| Milestone | Phase | Status | Completed |
|-----------|-------|--------|-----------|
| 1. Foundation (v1.2.0) | 1. Foundation Infrastructure | Completed | 2026-06-05 |
| 2. Reliability (v1.3.0) | 1. Error & Converter Hardening | Completed | 2026-06-05 |
| 3. Capability (v1.4.0) | 1. Features & Refactoring | Completed | 2026-06-06 |
| 4. External IdP (v1.5.0) | 1. Full External IdP Integration | Not started | - |
