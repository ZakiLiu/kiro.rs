# Scan Wisdom — Issues (RV-cred-review-2026-05-31)

## 不变量：usage 计数必须在 token 派发成功的每一条出口都发生
本次 starvation fix 把 record_usage 从 report_success 移到 acquire 派发点，但有**两条**派发出口：
- LB 出口 `token_manager.rs:1419`（已调 record_usage）
- affinity 命中出口 `token_manager.rs:1517`（**漏调** → COR-001）
凡未来再改 usage 计数位置，先 grep `return Ok(ctx)` 找全所有派发成功出口。

## 双套恢复机制，验证强度不同，别混淆
- `check_and_recover`（全局熔断恢复）：无验证、直接置 failure_count=0。本次放宽为恢复所有非 Manual/Suspended → 会无验证放行 402/auth 凭据 (SEC-001)。
- `get_recovery_candidates` + `start_periodic_recovery`（周期恢复）：对 Quota/Balance/Auth 类**先实际查余额/刷 token 验证**后才恢复。
新增 DisableReason 时，必须同时决定它在这两套机制里各自的归属。

## 退避调参权衡
退避上限 2h→30min、min(5)→min(3)：恢复更积极，但长期故障凭据后台探测频率上升（每凭据每小时约多 1.5 次上游 usage-limits/refresh 调用）→ PRF-001。

## [reviewer 确认] 死方法 ≠ 死变体：删 mark_insufficient_balance 但保留 DisableReason::InsufficientBalance
全仓 grep 确认：`mark_insufficient_balance` (2384) 无调用者（死代码），但 `DisableReason::InsufficientBalance` 变体仍活跃——
- 产生点：`token_manager.rs:2560`（余额初始化 remaining<1.0 自动禁用）仍可达
- 消费点：`provider.rs:291` 周期恢复按此 reason 查余额
即「方法死了但 reason 没死」。删方法时切勿连变体一起删，否则 2560/291 编译失败。这是 scanner 留给 reviewer 的悬念，已坐实。

## [reviewer 确认] 恢复机制其实是三套不是两套
除 check_and_recover（全局熔断,无验证）和 start_periodic_recovery（周期,按 reason 验证）外，还有第三套：
- `check_and_recover_individual` (token_manager.rs:2305)：请求时自愈，**仅** FailureLimit/RefreshFailureLimit，注释明示「其他类型由 start_periodic_recovery 处理，需要实际验证」。
这条注释正是 SEC-001 的判据——它说明项目早已确立「失败计数类可无验证自愈，余额/认证类必须验证」的分工，而 check_and_recover 的放宽恰好越过了这条线。修 SEC-001 时让全局版与 individual 版的排除原则对齐即可。
