# Scanner Learnings — RV-lb-sync-2026-05-31

## 项目关系判定（关键背景，REV 必读）

- current (`GitHub/kiro.rs`) 与 upstream (`GitHub/other/kiro.rs`) git 历史**完全分叉**，commit hash 无交集，不能 cherry-pick。
- **current 是 upstream 的超集**，证据：
  - upstream `src/kiro/endpoint/` 只有 `ide.rs`+`mod.rs`；current 还有 `cli.rs`。
  - upstream **无** cooldown.rs / affinity.rs / rate_limiter.rs / background_refresh.rs。
  - 行数对比：provider.rs current 2323 vs upstream 519；token_manager.rs current 4181 vs upstream 2599。
- 推论：upstream 是较老分支，current 从同源（hank9999 原作者）独立演进了整套 LB 体系。**同步方向基本为零**。
- 所有 upstream bugfix（551b91f 工具名63字符 / 2805585 invalid_grant / 298505b accessToken误禁用 / 53df562 profileArn动态注入）经逐一核验，current 均已覆盖或以更完善方式实现。

## 唯一 optional 同步项

- musl 静态编译 + rustls 双 CA 信任：current 用纯 rustls-tls（default-features=false），无 native-tls/musl 链路。属构建/部署能力差异，非运行时 bug。

## LB 饥饿根因（回答用户原始问题『部分账号长时间不能调用』）

核心嫌疑链 — F001（high）：余额卡 0<x<1.0 → 周期刷新主动禁用 → 恢复门槛同为 ≥1.0 → 指数退避最长 2h 静默。禁用阈值与恢复阈值都是 1.0，无滞后区间，边界反复抖动。

次要 — F002（medium）：recent_usage 只在 report_success 递增，429 凭据冷却到期后因 usage 假性偏低被反复优先选中再撞限流，反馈方向错误。

## 验证方法备忘

- `select_best_candidate_id` 第一优先级 `min(recent_usage)`，第二 `max(balance)`，全等则 round-robin。
- `record_usage` 唯一调用点在 `report_success`（token_manager.rs:2102），失败/429 路径不计 usage。
- 禁用-恢复门槛：disable `remaining < 1.0`（provider.rs:236），recover `remaining >= 1.0`（provider.rs:296），LOW_BALANCE_THRESHOLD=1.0。
- cooldown 短冷却 cap 300s，429 用 custom_duration 平坦冷却不累计 trigger_count（设计健康）。

## REV-001 增量发现（reviewer 源码深挖，scanner 未尽之处）

- **F001 真正的放大器是指数退避累加**：`mark_insufficient_balance`（token_manager.rs:2381）重置 attempts=0，但每次恢复探测失败 `increment_recovery_attempts_inner`（:2472 saturating_add）累加，`get_recovery_candidates`（:2414）据此 `5min*2^attempts.min(5)` 封顶 120min。余额卡 0~1.0 的账号探测间隔 5→10→20→40→80→120min，越关越久。最小修复一刀 = 阈值解耦+退避封顶 120→30min。

- **F002 决定性佐证在 token_manager.rs:3007-3016 注释**：项目方已明确认知 `min(recent_usage)` 选择策略会"猛派/雷暴"，并为**新凭据**用 max(recent_usage) baseline 做防护——但 429 冷却凭据无等价 usage 补偿，正是受害者。这条注释是判定"项目方已知此偏好缺陷"的关键证据。修复 = 派发计数与成功计数分离（acquire 时即记 usage）。

- **审查方法心得**：deep_analysis 两条均未走 CLI fan-out，reviewer 亲手追调用点闭环（record_usage 唯一调用者）+ 缓解机制注释，比 CLI 更扎实——这类语义判断 CLI 易漏。
