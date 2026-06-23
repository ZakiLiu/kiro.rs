# Harvest Report — 2026-06-23

## Source

本次收割扫描 7 个 artifact（全部，30 天窗口）：

| # | Source Type | ID | Title |
|---|-------------|-----|-------|
| 1 | analysis | analyze-P1-features-refactoring | M3 P1 Features & Refactoring 上下文 |
| 2 | lite-plan | plan-P1-features-refactoring | M3 P1: Prompt presets, PDF, 模块重构 |
| 3 | brainstorm | brainstorm-output-token-growth | Output Token 增长问题分析 |
| 4 | analysis | analyze-kiro-429-500-root-cause | 429/500 错误根因分析 |
| 5 | analysis | analyze-thinking-profilearn | Thinking mode & profileArn 移植分析 |
| 6 | lite-plan | plan-multi-api-endpoints | 多格式 API Endpoints 规划 |
| 7 | lite-plan | plan-port-ops-frontend | 运维管理 + 前端移植规划 |

## Extraction Summary

- Fragments found: 47
- Filtered by confidence (≥0.5): 0 dropped
- Duplicates skipped: 4（全部来自 brainstorm-output-token-growth，已在 spec 中记录）
- Routed items: 19（wiki 2 + spec 13 + issue 4）

## Routing Results

### Wiki (2 entries)

| # | Type | Slug / File | Title | Status |
|---|------|-------------|-------|--------|
| 1 | knowhow | KNW-harvest-429-root-cause-2026-06-23.md | 429/500 错误根因分析与修复策略 | CREATED |
| 2 | knowhow | KNW-harvest-thinking-port-2026-06-23.md | thinking mode 移植 GAP 分析 | CREATED |

> 注：`maestro wiki create` 仅允许 `spec` 类型写入（note/memory 不可写），故 wiki 条目以 `KNW-` knowhow 文件形式写入 `.workflow/knowhow/`，由 wiki indexer 自动收录。

### Spec (13 entries)

| # | Category | File | Title | Status |
|---|----------|------|-------|--------|
| 1 | debug | debug-notes.md | 429 路径缺少 backoff 导致 Thundering Herd | ADDED |
| 2 | debug | debug-notes.md | "do request failed" 500 非 kiro.rs 来源 | ADDED |
| 3 | learning | learnings.md | Output token 持续增长是正常行为 | ADDED |
| 4 | coding | coding-conventions.md | 429 重试策略：backoff + 阈值 + 总计数器 | ADDED |
| 5 | coding | coding-conventions.md | PDF 提取 feature-gated + 优雅降级 | ADDED |
| 6 | coding | coding-conventions.md | 模块拆分机械化 mod.rs re-export | ADDED |
| 7 | coding | coding-conventions.md | Prompt Preset 机制 x-preset-id | ADDED |
| 8 | coding | coding-conventions.md | contextUsageEvent 缓冲模式精确 input_tokens | ADDED |
| 9 | coding | coding-conventions.md | CLI endpoint 文件只读约束 | ADDED |
| 10 | arch | architecture-constraints.md | thinking mode 经 additionalModelRequestFields 传递 | ADDED |
| 11 | arch | architecture-constraints.md | THINKING_SIGNATURE_INVALID 自动重试一次 | ADDED |
| 12 | arch | architecture-constraints.md | 多格式 API Endpoints 架构 | ADDED |
| 13 | arch | architecture-constraints.md | 运维管理 + 前端移植架构决策 | ADDED |

### Issue (4 entries)

| # | Severity | ID | Title | Status |
|---|----------|-----|-------|--------|
| 1 | high | ISS-20260623-001 | 提高 input cache 命中率 | CREATED |
| 2 | medium | ISS-20260623-002 | 多格式 API Endpoints 实现 | CREATED |
| 3 | high | ISS-20260623-003 | 运维管理 + 前端移植实现（5 waves） | CREATED |
| 4 | high | ISS-20260623-004 | thinking mode GAP 实现 | CREATED |

> Issue 聚合策略：因多数 plan 任务的实现已在近期 commit（runtime model、prompt cache TTL 等）或 memory 记录中完成，9 个 wave 任务按 plan 聚合为 4 个宏观追踪项，标注需 triage 完成度。

## Skipped

| Fragment | Reason |
|----------|--------|
| Output 不可缓存 | Duplicate: architecture-constraints.md 已有「Kiro 上游无 token 计数」 |
| Output tokens 本地估算 | Duplicate: coding-conventions.md 已有「estimate_tokens 启发式精度限制」 |
| Thinking 计入 output_tokens | Duplicate: architecture-constraints.md 已有「Output/Thinking tokens 分离计数」且已修复 |
| Fix thinking tokens 单独计数 | Duplicate: 同上，已修复 |

## Notes

- 两个 plan（multi-api-endpoints / port-ops-frontend）的 wave 任务建议师兄 triage 后关闭已完成的 issue。
- wiki 连接（KNW ↔ spec ↔ issue）可在下次 `wiki-connect --fix` 时补全。
