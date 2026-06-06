# Roadmap: kiro-rs Fusion

## Overview

将 kiro-rs-dev-source 的核心能力（跨请求缓存、可观测性、统一错误映射、转换层增强、系统提示预设、PDF 支持）融入当前 kiro.rs 项目，同时完整保留现有竞争优势（压缩管道、Anti-Detection、图片处理、Web Portal、CLI 端点）。采用三阶段渐进式交付：Foundation (MVP) → Reliability → Capability，每个里程碑独立可交付、可验证。

## Milestones

### Milestone 1: Foundation — MVP (v1.2.0)
**Target**: 建立 CredentialIdentity 共享 trait、跨请求前缀缓存、请求指标可观测性三大基础能力
**Status**: planned

#### Phases

- [ ] **Phase 1: Foundation Infrastructure** — CredentialIdentity trait + CrossRequestCache + MetricsCollector

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

- [ ] **Phase 1: Error & Converter Hardening** — ErrorMapper + Tool Name Shortening + Forced Conversation ID

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

- [ ] **Phase 1: Features & Refactoring** — Prompt Presets + PDF Support + Module Decomposition

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

## Scope Decisions

- **In scope**: F-001 through F-008（8 个 feature），NFR-PERF-001 / NFR-SEC-001 / NFR-REL-001，Admin API 扩展
- **Deferred**: Prompt filter (prompt_filter.rs) — 需独立安全评估；token_manager.rs 内部拆分（可在 Milestone 3 后续版本进行）
- **Out of scope**: 前端 UI 重新设计、多区域部署、分布式缓存、多租户 SaaS 化、部署流程变更

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

## Progress

| Milestone | Phase | Status | Completed |
|-----------|-------|--------|-----------|
| 1. Foundation (v1.2.0) | 1. Foundation Infrastructure | Not started | - |
| 2. Reliability (v1.3.0) | 1. Error & Converter Hardening | Not started | - |
| 3. Capability (v1.4.0) | 1. Features & Refactoring | Not started | - |
