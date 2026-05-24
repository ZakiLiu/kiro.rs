---
related:
  - project-project
---
# Knowledge Digest: Full Wiki (all entries)
**Generated:** 2026-05-24 | **Entries:** 29 | **Health:** 93/100

## Baseline Metrics

| Metric | Value |
|--------|-------|
| Total entries | 29 |
| Broken links | 0 |
| Orphans | 7 |
| Hub nodes | 0 |
| Health score | 93/100 |

## Themes

### 1. Architecture & System Design (7 entries)

项目的骨架定义——从整体项目定位到模块划分、层级边界、依赖规则、技术约束和关键架构决策。知识覆盖完整，从宏观（单二进制部署、Axum + Tokio 运行时）到微观（serde preserve_order、subtle 常量时间比较）均有记录。

**Key entries:**
- `project-project` — 项目全景：需求、约束、技术栈、决策记录
- `spec:project:architecture-constraints` — 架构约束总纲
- `spec:project:architecture-constraints-002` — 层级边界（请求流转路径）
- `spec:project:architecture-constraints-003` — 依赖规则（单向依赖，common/model 无内部依赖）

**Gaps:**
- 无 knowhow 记录：缺少「为什么选择 Axum 而非 Actix」「为什么用 parking_lot」等决策推演过程
- 无 note 记录：缺少架构演进历史（如从单凭据到多凭据的迁移经验）
- 7 个 orphan 子条目未与其他主题建立关联

**Health:** 90/100 (orphans 拉低)

---

### 2. Backend Coding Conventions (8 entries)

Rust 后端编码规范全集——错误处理（thiserror + anyhow 分层）、命名（snake_case 函数 / PascalCase 类型）、异步模式（Tokio spawn + JoinHandle）、序列化（preserve_order + rename_all）、测试（#[tokio::test]）、日志（tracing 结构化）、安全（subtle + 输入验证）。

**Key entries:**
- `spec:project:coding-conventions` — 编码规范总纲
- `spec:project:coding-conventions-001` — 错误处理模式（thiserror 定义 + anyhow 传播）
- `spec:project:coding-conventions-007` — 安全编码（常量时间比较、输入验证、脱敏）
- `spec:project:coding-conventions-003` — 异步模式（spawn 管理、取消安全）

**Gaps:**
- 无 knowhow：缺少「常见 Rust 编译错误及解法」「性能调优经验」等实操知识
- 无 issue：未记录已知的代码质量问题或技术债
- 安全规范有条目但缺少攻防验证记录

**Health:** 95/100

---

### 3. Testing & Quality (6 entries)

测试策略完整定义——测试类型（单元/集成/E2E）、覆盖目标（核心逻辑 80%+）、测试模式（builder pattern、fixture）、排除项（不测 serde derive、不测 Axum 路由）、运行方式。

**Key entries:**
- `spec:project:testing-strategy` — 测试策略总纲
- `spec:project:testing-strategy-001` — 测试类型定义与边界
- `spec:project:testing-strategy-003` — 测试模式（builder、fixture、mock boundary）

**Gaps:**
- 无 knowhow：缺少「如何 mock Kiro API 响应」「流式测试技巧」等实操 recipe
- 无 issue：未记录测试覆盖率现状或已知测试盲区
- 缺少 CI/CD 集成测试的运行环境说明

**Health:** 95/100

---

### 4. Frontend/UI Conventions (6 entries)

React 前端规范——设计系统（暗色主题、Tailwind 色板）、组件模式（函数组件 + hooks）、状态管理（React Query + Zustand）、API 集成（fetch wrapper + error boundary）、命名规范（PascalCase 组件 / kebab-case 文件）。

**Key entries:**
- `spec:project:ui-conventions` — UI 规范总纲
- `spec:project:ui-conventions-001` — 设计系统（色板、字体、间距）
- `spec:project:ui-conventions-002` — 组件模式（composition over inheritance）
- `spec:project:ui-conventions-003` — 状态管理（React Query 服务端 + Zustand 客户端）

**Gaps:**
- 无 knowhow：缺少「Admin UI 常见交互模式」「表单验证最佳实践」等前端 recipe
- 无 issue：未记录已知 UI bug 或体验问题
- 缺少可访问性（a11y）规范

**Health:** 95/100

---

## Coverage Heatmap

```
              Architecture   Coding Conv.   Testing   Frontend/UI
spec          █████          █████          █████     █████
knowhow       ░░░░░          ░░░░░          ░░░░░     ░░░░░
note          ░░░░░          ░░░░░          ░░░░░     ░░░░░
issue         ░░░░░          ░░░░░          ░░░░░     ░░░░░

Legend: █ = entries exist (3+), ▓ = sparse (1-2), ░ = empty (0)
```

**诊断**: 知识图谱呈现典型的「初始化完成、经验未沉淀」状态。所有结构性规范（spec）已就位，但实操经验（knowhow）、观察笔记（note）、问题追踪（issue）三个维度完全空白。

## Knowledge Gaps

| Gap | Theme | Type Missing | Suggested Action |
|-----|-------|-------------|-----------------|
| 无架构决策推演记录 | Architecture | knowhow | `/learn-decompose src/` 提取决策上下文 |
| 无 Rust 实操 recipe | Coding Conv. | knowhow | `maestro wiki create --type knowhow --slug rust-error-patterns` |
| 无测试实操指南 | Testing | knowhow | `/learn-decompose src/` 提取测试模式 |
| 无前端开发 recipe | Frontend/UI | knowhow | `maestro wiki create --type knowhow --slug admin-ui-patterns` |
| 无已知问题追踪 | All | issue | 从 git log / GitHub issues 导入 |
| 无开发笔记 | All | note | 日常开发中随手记录 gotcha |
| 7 个 orphan 条目 | Architecture | (connectivity) | `/wiki-connect --fix` 建立关联 |
| 无 hub 节点 | All | (connectivity) | 为核心条目添加 `related` 链接 |

## Unlinked Insights

`specs/learnings.md` 存在但为空——尚无可交叉引用的经验条目。

## Recommended Actions

1. **`/learn-decompose src/`** — 从源码中提取隐含的架构决策和编码模式，生成 knowhow 条目
2. **`/wiki-connect --fix`** — 修复 7 个 orphan 条目的连接关系，提升图谱连通性
3. **日常沉淀** — 开发过程中遇到的 gotcha、调试经验、性能发现随手记录为 note/knowhow
4. **从 git history 回溯** — 重要 commit 的决策背景可补录为 knowhow/decision 条目
5. **可访问性规范** — Frontend/UI 主题缺少 a11y 相关规范，建议补充
