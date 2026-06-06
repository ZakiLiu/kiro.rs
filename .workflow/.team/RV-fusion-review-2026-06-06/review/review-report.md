# Fusion Review Report — kiro.rs

- **Review ID**: RV-fusion-review-2026-06-06
- **Date**: 2026-06-06
- **Target**: `git diff 45c62f7..HEAD` (40 files, +8417/-5656)
- **Findings**: 9（全部源码核实 verified=9/9）

## Executive Summary

本轮大改动为 `token_manager.rs`、`converter/`、`stream/`、`error_map` 的模块拆分 + 新增跨请求缓存。重构整体质量良好（错误映射提取为纯等价、单测覆盖完整、clippy 0 error）。

但发现 **1 个 critical**：跨请求缓存集成时把 `credential_id` 写死为 `0`，使精心设计的 per-credential 隔离完全失效。其余为 1 high（PDF 内存放大 DoS）、4 medium、3 low。

**核心结论**：critical 与多个 maintainability/performance 发现**同源同文件**——修 SEC-001 的同一次重构可顺带关闭 MNT-002、为 PRF-001 创造重写时机。错误处理类问题（COR-002 + SEC-003）也集中在 `error_map.rs`，可一并收敛。

## Metrics — Dimension × Severity

| Dimension | critical | high | medium | low | 小计 |
|-----------|:--------:|:----:|:------:|:---:|:---:|
| Security        | 1 | 1 | 0 | 1 | 3 |
| Correctness     | 0 | 0 | 2 | 0 | 2 |
| Performance     | 0 | 0 | 1 | 0 | 1 |
| Maintainability | 0 | 0 | 1 | 2 | 3 |
| **合计**        | **1** | **1** | **4** | **3** | **9** |

- fixable: 9/9 · auto-fixable（clippy --fix 等）: 3（MNT-001, MNT-003, + COR-001 单行）

## Critical / High Findings

| ID | Sev | 标题 | 位置 | 修复 |
|----|-----|------|------|------|
| SEC-001 | critical | 跨请求缓存写死 `credential_id=0`，隔离失效，conversation_id 跨凭据串话 | handlers.rs:979, :1132 | refactor / medium |
| SEC-002 | high | PDF 先整体 base64 解码再查大小，内存放大 DoS | pdf.rs:61 | minimal / low |

### SEC-001 深挖（critical 边界澄清）

- **铁证**：`handlers.rs:979 cache.lookup(0, fp)` + `:1132 cache.insert(0, *fp, conv_id)`，两处写死 0。CacheKey 隔离维度坍缩为仅 content_fingerprint。
- **实际利用面**（关键边界）：conversation_id 有三条派生路径——
  - (b) **内容指纹派生**：两请求同内容 → 本就派生相同 conv_id → 注入无害。
  - (a) **metadata.user_id 携带 session UUID** / (c) **空 history 走 Uuid::v4()**：**非内容确定性** → credential A 缓存的 conv_id 被 credential B 命中注入 → **真串话**。
- 即便利用面被收窄，隔离机制对外宣称生效而实则失效，且会污染上游 prompt cache 归属，须修。
- **修复时序坑**：凭据在 `provider.call_api` 内部按优先级/故障转移选定，而 `lookup` 发生在选凭据**之前**。最小修复至少保证 `insert` 用真实 id、`lookup` 同维度，否则隔离仍部分失效。建议把 key 改为 `(cache_identity, fp)` 并在入口确定。

## Critical Files（≥2 维度 / 多发现）

| 文件 | 发现 | 维度 | 说明 |
|------|------|------|------|
| `cross_request_cache.rs` | SEC-001, PRF-001 | security, performance | 隔离失效根因 + O(n) 热路径，建议同次重写 |
| `identity.rs` | SEC-001, MNT-002 | security, maintainability | cache_identity() 应用未用 + 死抽象本体 |
| `error_map.rs` | COR-002, SEC-003 | correctness, security | 字符串耦合 + 错误详情泄露 |
| `handlers.rs` | SEC-001 | security | SEC-001 触发点 |

## Root Cause Groups

1. **cache-isolation-broken**（primary: **SEC-001**，members: SEC-001 / MNT-002 / PRF-001）
   写死 `credential_id=0` 是主根因。MNT-002（cache_identity 死抽象）是其直接症状；PRF-001 同文件。**修 SEC-001 = 同时关掉 MNT-002，并为 PRF-001 重写创造时机。**

2. **error-handling-coupling**（primary: **COR-002**，members: COR-002 / SEC-003）
   分类靠中文魔法字符串耦合 token_manager（COR-002），对外又泄露上游 err（SEC-003）。同在 error_map.rs，引入结构化错误类型可一并收敛。

## Optimization Suggestions

| 优先级 | 建议 | 覆盖 |
|:---:|------|------|
| P0 | 修 SEC-001 时重写 cache key 为 `(cache_identity, fp)` + 换 `lru` crate，一次解决隔离/死抽象/O(n) | SEC-001, MNT-002, PRF-001 |
| P0 | PDF 解码前长度预检 `(base64_len/4*3)>10MB` 即拒（O(1)） | SEC-002 |
| P1 | token_manager 结构化错误 enum，classify 基于类型；对外统一通用文案、详情入日志 | COR-002, SEC-003 |
| P1 | token_usage 解析失败 `Err` 分支加 `tracing::warn!`（零行为变更） | COR-001 |
| P2 | `cargo clippy --fix` 清理 unused import + 风格 | MNT-001, MNT-003 |

## Recommended Fix Scope

- **Must fix**: SEC-001（隔离失效）、SEC-002（DoS，修复成本极低）
- **Should fix**: COR-001、COR-002、SEC-003
- **Nice to have（建议绑定 SEC-001 重构）**: PRF-001、MNT-001、MNT-002、MNT-003

> 注：SEC-001 实际利用面受 conv_id 派生分支限制，但属"宣称隔离实则失效"，定级 critical 合理。MNT-002/PRF-001 与 SEC-001 同源，建议一次重构闭环。
