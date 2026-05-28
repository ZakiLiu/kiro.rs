---
related:
  - project-project
  - knowhow-digest-full-2026-05-24
  - knowhow-periodic-recovery-2026-05-25
---
# Knowledge Digest: Full Wiki (all entries)
**Generated:** 2026-05-25 | **Entries:** 91 | **Health:** 99/100

## Baseline Metrics

| Metric | Value |
|--------|-------|
| Total entries | 91 |
| Types | spec: 35, knowhow: 55, project: 1 |
| Health score | 99/100 |
| Broken links | 0 |
| Orphans | 1 |
| Top hub | spec:project:coding-conventions (in-degree: 6) |

## Themes

### 1. Resilience & Credential Management (18 entries)

项目的核心架构主题。凭据故障转移链、指数退避+抖动、Circuit Breaker（全局禁用）、Cooldown 分级退避、错误分类守卫——这些模式共同构成了「不丢请求」的韧性保障。最新的周期性凭据恢复机制（`start_periodic_recovery`）和后台 Token 刷新（`start_background_token_refresh`）进一步补全了凭据生命周期管理的闭环。

**Key entries:**
- `knowhow-KNW-decompose-src-2026-05-24-022` — E1. Retry Loop with Exponential Backoff + Jitter
- `knowhow-KNW-decompose-src-2026-05-24-023` — E2. Circuit Breaker via Global Credential Disable ⭐
- `knowhow-KNW-decompose-src-2026-05-24-024` — E3. Credential Failover with Affinity-Aware Exclusion ⭐
- `knowhow-KNW-decompose-src-2026-05-24-025` — E4. Differentiated Cooldown with Exponential Backoff per Reason
- `knowhow-KNW-follow-provider-2026-05-24-008` — Resilience Architecture Summary

**Gaps:**
- 缺少 `note` 类型条目记录实际故障案例（postmortem）
- 新增的 `start_periodic_recovery` 和 `start_background_token_refresh` 尚未被 wiki 收录
- `increment_recovery_attempts` 持久化策略的 trade-off 决策未记录

**Health:** 92/100 (缺少实操 note，新功能未同步)

---

### 2. Architecture & Module Design (11 entries)

模块分层清晰：anthropic/ → kiro/ → common/，依赖单向流动。Layer Boundaries 图准确描述了请求路径。Technology Constraints 记录了 rustls、parking_lot、subtle 等关键选型。

**Key entries:**
- `spec:project:architecture-constraints` — 总纲（in-degree: 3）
- `spec:project:architecture-constraints-002` — Layer Boundaries
- `spec:project:architecture-constraints-005` — Key Architectural Decisions

**Gaps:**
- 缺少 `knowhow` 条目解释为什么选择 parking_lot 而非 tokio::sync::Mutex
- 无 `issue` 类型条目追踪已知架构债务（如 client_cache 无驱逐策略）

**Health:** 88/100 (纯 spec，缺乏经验层和问题追踪)

---

### 3. Protocol & Data Processing (14 entries)

双向协议转换管道（Anthropic ↔ Kiro）、AWS Event Stream 二进制帧解析、SSE 状态机、JSON Schema 标准化、GIF 自适应抽帧——这些是项目的技术深水区。模式分解覆盖充分。

**Key entries:**
- `knowhow-KNW-decompose-src-2026-05-24-014` — D1. Bidirectional Protocol Translation Pipeline ⭐
- `knowhow-KNW-decompose-src-2026-05-24-017` — D4. AWS Event Stream Binary Frame Parser
- `knowhow-KNW-decompose-src-2026-05-24-008` — B1. SSE State Machine

**Gaps:**
- 缺少 WebSearch 工具路由的模式文档
- 输入压缩管道（B4）的阈值调优经验未记录为 note

**Health:** 95/100 (knowhow 覆盖好，缺少运维经验)

---

### 4. Frontend & UI Conventions (9 entries)

React 18 + TypeScript + Tailwind + shadcn/ui 技术栈。组件库、状态管理（Zustand）、文件组织、命名规范均有 spec 覆盖。

**Key entries:**
- `spec:project:ui-conventions` — 总纲
- `spec:project:coding-conventions-008` — Frontend Patterns

**Gaps:**
- 零 knowhow 条目——前端没有模式分解
- 无 admin-ui 组件的实际开发经验记录
- 缺少前端构建与 rust-embed 集成的 gotcha 记录

**Health:** 75/100 (纯规范，零实操)

---

### 5. Testing & Quality (6 entries)

测试框架（内置 `#[test]` + tokio::test）、目录结构、命名规范、模式（builder pattern for fixtures）均有 spec。Quality Rules 定义了工具链和构建顺序。

**Key entries:**
- `spec:project:test-conventions` — 总纲
- `spec:project:quality-rules` — 质量规则

**Gaps:**
- 零 knowhow 条目——无实际测试编写经验
- 无集成测试策略文档（如何 mock Kiro API？）
- CI/CD pipeline 配置未记录

**Health:** 70/100 (纯规范，零实操)

---

## Coverage Heatmap

```
              Resilience  Architecture  Protocol  Frontend  Testing
spec          ███░░       █████         ██░░░     █████     █████
knowhow       █████       ░░░░░         █████     ░░░░░     ░░░░░
note          ░░░░░       ░░░░░         ░░░░░     ░░░░░     ░░░░░
issue         ░░░░░       ░░░░░         ░░░░░     ░░░░░     ░░░░░

Legend: █ = entries exist, ░ = sparse/missing
```

## Knowledge Gaps

| Gap | Theme | Type Missing | Suggested Action |
|-----|-------|-------------|-----------------|
| 无故障案例记录 | Resilience | note | 下次线上事故后记录 postmortem |
| 新增恢复机制未入 wiki | Resilience | knowhow | `maestro wiki create --type knowhow --slug periodic-recovery` |
| client_cache 无驱逐 | Architecture | issue | `maestro wiki create --type issue --slug client-cache-eviction` |
| 前端零实操 | Frontend | knowhow | `/learn-decompose admin-ui/src/` |
| 测试零实操 | Testing | knowhow | 编写测试时随手记录 gotcha |
| 全局无 note 类型 | All | note | 日常开发遇到 gotcha 时用 note 记录 |
| 全局无 issue 类型 | All | issue | 用 issue 追踪已知技术债 |

## Unlinked Insights

以下 learnings.md 条目与 wiki 主题相关但未被引用：
- learnings-003 提到「三个 cooldown 变体已定义但未接线」→ 应创建 issue 追踪
- learnings-004 提到「is_rate_limit_response 是 dead code」→ 应创建 issue 追踪
- learnings-004 提到「MAX_TOTAL_RETRIES=3 在大凭据池场景可能不足」→ 应创建 issue 追踪

## Recommended Actions

1. **记录新增功能**: 将 `start_periodic_recovery` 和 `start_background_token_refresh` 的设计决策写入 wiki knowhow
2. **前端模式分解**: `/learn-decompose admin-ui/src/` 填补前端 knowhow 空白
3. **创建 issue 追踪技术债**: client_cache 驱逐、dead code 清理、cooldown 未接线
4. **引入 note 类型**: 下次遇到 gotcha 或线上问题时用 `maestro wiki create --type note` 记录
5. **修复 orphan**: 当前 1 个 orphan，运行 `/wiki-connect --fix` 消除
