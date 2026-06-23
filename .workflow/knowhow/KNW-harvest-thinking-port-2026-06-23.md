---
related:
  - "spec:project:architecture-constraints"
  - knowhow-knw-follow-provider-2026-05-24
---

# thinking mode 移植 GAP 分析（additionalModelRequestFields/profileArn）

## 背景

对比参考项目 Kiro-account-manager，kiro.rs 在 thinking mode 传递上有 4 个 GAP。

## GAP 清单

| GAP | 内容 | 优先级 |
|-----|------|--------|
| GAP-1 | KiroRequest 缺 `additionalModelRequestFields` 字段（仅有 conversation_state + profile_arn），无法传递 thinking 配置 | 高 |
| GAP-2 | 缺 budget_tokens → effort 映射：≤4000→low, ≤16000→medium, ≤64000→high, else→xhigh | 高 |
| GAP-3 | THINKING_SIGNATURE_INVALID 无自动重试——参考项目会剥离 history reasoningContent 后重试，kiro.rs 直接报错给客户端 | 中 |
| GAP-4 | profileArn 占位符策略差异：参考项目对 BuilderId 始终发占位 ARN，kiro.rs IDE 端点在 BuilderId/IdC 时移除 profileArn | 延期 |

## 决策

- additionalModelRequestFields 类型用 `Option<serde_json::Value>` 保持灵活。
- effort 阈值沿用参考项目。
- SIGNATURE_INVALID 重试仅一次。
- GAP-4 暂不改：当前策略经充分测试且功能正常，贸然改动风险 > 收益。

## 实现顺序

优先 GAP-1 + GAP-2（基础设施），其次 GAP-3（鲁棒性），GAP-4 延期。追踪见 ISS-20260623-004。

来源：.workflow/scratch/20260614-analyze-thinking-profilearn/
