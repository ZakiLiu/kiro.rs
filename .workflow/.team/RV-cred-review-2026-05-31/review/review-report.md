# 代码审查报告 — 凭据负载均衡/恢复重构

- **Review ID**: RV-cred-review-2026-05-31
- **日期**: 2026-05-31
- **范围**: `HEAD~5..HEAD`，6 个 Rust 源文件（starvation fix + 退避调整 + 主动禁用移除 + Opus 4.8 支持）
- **审查方式**: reviewer 对全部 6 项发现逐行 trace 源码（token_manager.rs + provider.rs），未走 CLI fan-out
- **工具链**: clippy clean（5 个既有 warning，非本次逻辑）；cargo test 9 项绿（context_window + map_model）

## 结论

**CHANGES_RECOMMENDED** — 合入前必修 1 项，建议修 2 项。

> starvation fix 的核心不变量「派发即计数」在 **affinity 主路径上被绕过**（COR-001, HIGH）——这恰好是本次 fix 想达成的目标，却在最高频的连续对话路径上失效。配合一处熔断恢复无验证放行（SEC-001, MED），需处理后合入。
>
> **`token_manager.rs` 是绝对热点：6 项发现中 5 项落在此文件，横跨全部 4 个维度。**

## 指标矩阵

| 维度 \ 严重度 | HIGH | MEDIUM | LOW | 小计 |
|---|---|---|---|---|
| correctness | 1 | 0 | 1 | 2 |
| security | 0 | 1 | 0 | 1 |
| performance | 0 | 1 | 0 | 1 |
| maintainability | 0 | 0 | 2 | 2 |
| **小计** | **1** | **2** | **3** | **6** |

- 可修复：3 / 6 ｜ 低风险可自动修：2（COR-001, SEC-001）
- 合入前必修：COR-001 ｜ 建议修：SEC-001, MNT-001 ｜ 可选/延后：PRF-001, MNT-002 ｜ 无需动作：COR-002

## HIGH / MEDIUM 发现

### COR-001 [HIGH] 亲和性派发路径未调用 record_usage — `token_manager.rs:1517`
**已验证**：LB 路径 `:1419` 成功后立即 `record_usage(id)`；affinity 命中分支 `:1515-1518` 成功后只 `affinity.touch()` 就 return，**漏了 record_usage**。
**根因**：usage 计数前移到派发点时只补了 LB 出口，遗漏 affinity 出口。更隐蔽：affinity 分支在 `:1496` 已通过 `rate_limiter.try_acquire` 真实消耗速率配额，却不计 usage——LB 视角下该凭据 `recent_usage` 恒为 0。
**影响（high）**：(1) `select_best_candidate_id` 的 min_usage 负载均衡被持续带偏，affinity 失效回落 LB 后该凭据因 usage=0 被反复优先选中，**与 fix 不变量直接冲突**；(2) 余额动态 TTL 误判低频用户（30min 而非 10min）。连续对话几乎全量命中 affinity，意味着主路径大面积不计 usage。
**修复**：minimal / low — `:1517` 补 `self.record_usage(bound_id)`；可选彻底做法是收敛到 `try_ensure_token` 成功单一出口（需确认无非派发调用方）。

### SEC-001 [MEDIUM] 熔断恢复无验证放行 QuotaExceeded/AuthFailed — `token_manager.rs:2274`
**已验证**：`check_and_recover` (2272-2287) 仅排除 Manual/AccountSuspended，其余一律无验证 `disabled=false + failure_count=0`。对比 `provider.rs:291-345` 周期恢复对余额类查余额、认证类刷 token 后才恢复。另：`check_and_recover_individual` (2305) 注释明示「其他类型由 start_periodic_recovery 处理，需要实际验证」——该分工约定被全局版破坏。
**根因**：三套恢复机制验证策略不一致，全局熔断版把「主因 ModelUnavailable」泛化为「恢复所有 reason」。
**影响（medium）**：仅熔断窗口期（~5min）非常态。被误放的 QuotaExceeded/AuthFailed 凭据下一请求大概率再撞 402/401，增上游调用 + 故障转移开销，可能在用户路径多一跳失败延迟。
**修复**：minimal / low — 排除列表追加 QuotaExceeded/InsufficientBalance/AuthenticationFailed。

### PRF-001 [MEDIUM] 退避上限 2h→30min 抬高探测负载 — `token_manager.rs:2422`
**已验证**：`(5 * (1<<min(attempts,3))).min(30)` → 序列 5,10,20,30,30...；旧 5,10,20,40,80,120。
**根因**：显式权衡——为配合可用性目标缩短退避，副作用是长期失败凭据探测从 2h/次 升到 30min/次。
**影响（medium）**：每长期失败凭据 +~1.5 次/小时上游 usage-limits/refresh 调用，N 凭据则 +1.5N/小时后台负载。与 SEC-001 叠加（误放→再禁用→进 30min 探测循环）相互放大。
**修复**：skip（可选）/ medium — 若上游无限流压力可 accept；否则对 attempts>=3 回到 60min 降频。**需上游限流数据裁决。**

## LOW 发现（pass-through）

| ID | 维度 | 标题 | 处置 |
|---|---|---|---|
| COR-002 | correctness | let-chain 重构语义等价 | 无需修改（已验证等价） |
| MNT-001 | maintainability | `mark_insufficient_balance` 死代码 | minimal — 删方法，**保留 `DisableReason::InsufficientBalance` 变体**（line 2560 + provider.rs:291 仍活跃，已确认 2560 可达） |
| MNT-002 | maintainability | 模型定义四处分散 | 延后技术债 — 长期可建模型注册表 |

## 关键文件

| 文件 | 命中数 | 维度 | 说明 |
|---|---|---|---|
| `src/kiro/token_manager.rs` | 5 | 全 4 维 | 本次变更绝对核心。派发计数/熔断恢复/退避/死代码集中于此。fixer 须视为单点高风险，改完回归 **LB / affinity / recovery 三条路径**。 |
| `src/kiro/provider.rs` | 2 | sec, prf | 带验证的周期恢复对照路径（291-345）。修 SEC-001 的前提是理解此路径。 |

## 根因聚类

- **RCG-1（主因 COR-001）**：「派发即计数」只在 LB 出口落地，遗漏 affinity 出口；affinity 还消耗 rate_limit 配额却不计 usage。
- **RCG-2（主因 SEC-001，含 PRF-001）**：三套恢复机制并存、验证与频率策略未统一。SEC-001 越权恢复需验证的 reason，PRF-001 缩短退避抬高探测——误放凭据立即再禁用并掉进 30min 探测循环，相互放大。

## 优化建议（按优先级）

| 优先级 | 来源 | 建议 |
|---|---|---|
| **P0** | COR-001 | `:1517` 补 `record_usage(bound_id)`；考虑收敛到 `try_ensure_token` 单一出口 |
| **P1** | SEC-001 | `check_and_recover` 排除列表追加余额/认证类 reason |
| **P2** | MNT-001 | 删 `mark_insufficient_balance`，保留 `InsufficientBalance` 变体 |
| P3 | PRF-001 | 可选：attempts>=3 回到 60min 退避；需上游限流数据 |
| defer | MNT-002 | 长期：单一模型注册表 |

## 给 fixer 的建议修复范围（FIX-001）

1. **必修** COR-001（P0，1 行）
2. **建议同批** SEC-001（P1，改 match）、MNT-001（P2，删死方法）
3. **裁决项** PRF-001 默认 skip（accept 现状），除非有上游限流证据
4. MNT-002 不在本次范围
5. 三项改动全集中在 `token_manager.rs`，**改完务必回归 LB / affinity / recovery 三条路径，并复跑 context_window + map_model 测试**
