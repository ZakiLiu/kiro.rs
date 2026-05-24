---
title: Learnings
readMode: optional
priority: medium
category: learning
keywords:
  - bug
  - lesson
  - gotcha
  - learning
related:
  - knowhow-decompose-src-2026-05-24
  - knowhow-follow-provider-2026-05-24
---


# Learnings

Bugs, gotchas, and lessons learned during development.
Add entries with: `/spec-add learning <description>`

## Entries

<spec-entry source="wiki-digest" category="technique" date="2026-05-24">
Wiki 初始化后知识图谱呈「全 spec 零经验」状态（29 条 spec，0 条 knowhow/note/issue）。结构性规范完备但缺乏实操沉淀。优先动作：从源码提取隐含决策（/learn-decompose）、修复 orphan 连接（/wiki-connect --fix）、日常开发随手记录 gotcha。
</spec-entry>

<spec-entry source="wiki-connect" category="technique" date="2026-05-24">
wiki-connect --fix 将健康分从 93 提升到 100/100，消除全部 10 个 orphan。关键策略：(1) knowhow↔spec type bridge 是最高价值连接（让经验层和规范层互通）；(2) coding-conventions 自然成为 hub（in-degree 6），符合其「总纲」定位；(3) project.md 只读限制导致 project-project 无法被充分连接，需通过其他条目的 related 间接引用。
</spec-entry>

<spec-entry source="learn-decompose" category="technique" date="2026-05-24">
src/ 全量模式分解：63 文件 → 40 raw patterns → 24 unique（6 已记录、5 跨维度合并）。核心发现：(1) 韧性是主导架构主题（retry/failover/circuit-breaker/cooldown/compression/decoder-recovery 全服务于「不丢请求」）；(2) 协议隔离严格（Anthropic 类型不泄漏到 Kiro 类型）；(3) 自适应优于固定值（TTL/压缩阈值/cooldown/采样率均动态调整）；(4) 字符串错误分类是已知技术债；(5) 三个 cooldown 变体已定义但未接线。
</spec-entry>

<spec-entry source="learn-follow" category="technique" date="2026-05-24">
provider.rs 韧性链路深度阅读：(1) 错误分类决定重试策略——400 bail、401/403 failover、402 disable+failover、429 cooldown、5xx retry、network retry-without-failover（Round 11 决议）；(2) client_cache 无驱逐策略是潜在内存泄漏；(3) MAX_TOTAL_RETRIES=3 硬上限在大凭据池场景可能不足；(4) Round 注释系统记录了迭代决策历史，是重要的考古线索；(5) is_rate_limit_response 是 Round 8 后的 dead code，保留但未接入。
</spec-entry>

