---
related:
  - "spec:project:debug-notes"
  - "spec:project:coding-conventions"
  - knowhow-knw-follow-provider-2026-05-24
  - knowhow-knw-periodic-recovery-2026-05-25
---

# 429/500 错误根因分析与修复策略

## 根因结论

1. **429 雪崩 = Thundering Herd**：凭据轮转重试循环的 429 路径用 `continue` 直接跳下一个凭据，缺少 `sleep(retry_delay())`（5xx 路径有）。上游限流时所有凭据被无延迟连续轮转，放大成雪崩。这是真正可修复的 bug。
2. **500s 是上游真实错误**：MODEL_TEMPORARILY_UNAVAILABLE 已正确重试 + 故障转移，无需改动。
3. **"do request failed" 500 非本仓库**：该错误消息在 kiro.rs 全量源码中搜不到，判定为历史遗留或 sub2api 项目错误，非当前代码问题。

## 修复策略（三件套）

- **加 backoff**：MCP + streaming 两处 429 分支 `continue` 前插入 `sleep(Self::retry_delay(attempt))`，与 5xx 一致。
- **固定连续阈值**：连续 429 阈值从 `(available/2).max(MIN)` 改为固定 `3`——3 个不同凭据连续 429 即全局限流确证。
- **总计数器**：新增不重置的 `total_429_count`，阈值 5——捕获 500/网络错误与 429 交替的边缘场景（consecutive 会被打断归零，total 不会）。

## 方法论

- **对称性检查**：同类错误处理路径（429 vs 5xx）应有对称的退避逻辑，不对称即隐患。
- **计数器语义**：consecutive（可重置）与 total（不可重置）解决不同问题——前者判瞬时风暴，后者判长尾累积。

来源：.workflow/scratch/20260609-analyze-kiro-429-500-root-cause/
