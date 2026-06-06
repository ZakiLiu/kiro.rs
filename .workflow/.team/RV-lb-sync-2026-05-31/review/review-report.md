# 审查报告 — 负载均衡饥饿 + 上游同步

- **Review ID**: REV-001
- **日期**: 2026-05-31
- **目标**: load-balancing + upstream-sync
- **扫描来源**: `.workflow/.team/RV-lb-sync-2026-05-31/scan/scan-results.json`
- **方法**: 人工源码逐行追踪验证（未走 CLI fan-out —— 5 个发现全部由 reviewer 亲自比对当前源码确认）

## 执行摘要

Scanner 发现的 5 个问题**全部经源码验证属实**，分析准确。核心结论：

1. **F001（HIGH）是用户报告『部分账号长时间不能调用』的真正主因**，必修。根因不止是阈值无滞后，更狠的是**指数退避在恢复探测连续失败时累加到 2 小时静默**——余额卡在 0~1.0 的账号会被越关越久。
2. **F002（MEDIUM）有一处关键佐证**：项目方在 `token_manager.rs:3007` 的注释里已经明确知道 `min(recent_usage)` 选择策略会造成"猛派/雷暴"，并为**新凭据**做了 max-baseline 防护——但 **429 冷却凭据没有等价的 usage 补偿**，正是这套机制下被反复撞限流的受害者。根因分析站得住。
3. **上游同步方向基本为零**：current 是 upstream 的超集，所有 bugfix 已独立覆盖且更完善。唯一 optional 是 musl 静态编译链（构建能力，非运行时 bug）。
4. **无安全（sec）维度发现**。

## 维度 × 严重度矩阵

| 维度 \ 严重度 | critical | high | medium | low |
|---|---|---|---|---|
| cor（正确性） | 0 | 1 (F001) | 1 (F002) | 1 (F004) |
| prf（性能） | 0 | 0 | 0 | 1 (F003) |
| sec（安全） | 0 | 0 | 0 | 0 |
| mnt（可维护） | 0 | 0 | 0 | 1 (F005) |

可修复 4 项，其中 2 项（F001/F002）为高价值修复。

## High / Medium 发现详表

| ID | 维度 | 严重度 | 标题 | 根因（一句话） | 修复策略 | 复杂度 |
|---|---|---|---|---|---|---|
| F001 | cor | **high** | InsufficientBalance 凭据陷入禁用→指数退避→最长 2h 静默 | 禁用/恢复阈值同为硬 1.0 无滞后 + 恢复探测失败累加 attempts → 退避翻倍到 120min | minimal | low |
| F002 | cor | medium | 429 冷却凭据到期后因 usage 假性偏低被反复优先选中再撞限流 | record_usage 仅成功路径递增，429 冷却凭据 usage 冻结 → 到期成 min(usage) 首选 → 再撞 429 | refactor | medium |

### F001 根因详解

- **触发**：`provider.rs:236` 周期 balance 刷新 `if remaining < 1.0 { mark_insufficient_balance(id) }`。
- **放大器**：`mark_insufficient_balance`（`token_manager.rs:2381`）把 `recovery_attempts` 重置为 0；但每次恢复探测余额仍 < 1.0 时 `increment_recovery_attempts_inner`（`:2472` saturating_add）累加；`get_recovery_candidates`（`:2414`）据此做 `5min * 2^attempts.min(5)` 封顶 **120min** 退避。
- **后果**：余额卡在 0<x<1.0 的账号，恢复探测每次失败 → 探测间隔 5→10→20→40→80→120min，第 6 次起恒定 **2 小时**完全不可调用。Opus 这类大计费单位模型 remaining<1.0 仍可能够 1 次调用，却被过早硬禁用。
- **影响面**：high — 凭据越多、越接近月末，受影响账号越多。

### F002 根因详解

- `select_best_candidate_id` 第一优先级 `min(recent_usage)`（`token_manager.rs:1142`）。
- `record_usage` 唯一调用点是 `report_success → record_usage`（`:2102`）。429 路径 `set_credential_cooldown_with_duration`（`provider.rs:1027`）只设冷却、**不计 usage**。
- 冷却期间 usage 冻结 → 到期立即成全局 min → 再被猛派 → 再撞 429 → 反馈环。
- **佐证**：`token_manager.rs:3007-3016` 注释明确为**新凭据**用 `max(recent_usage)` baseline 防"雷暴"，证明项目方已知此偏好缺陷，但 429 凭据无对应补偿。

## Low 发现（pass-through，已验证）

| ID | 维度 | 标题 | 处置建议 |
|---|---|---|---|
| F003 | prf | usage 重置窗口不同步造成周期性突发不公平 | 可并入 F002 重构（衰减计数替代硬重置） |
| F004 | cor | 全局熔断恢复只恢复 ModelUnavailable，窗口期内改写原因的凭据漏恢复 | 搭车 F001 一起改恢复路径 |
| F005 | mnt | map_model 未知 sonnet 版本兜底 4.5 vs 上游严格 None | 有意设计，仅文档化，无需同步 |

## 关键文件（跨维度热点）

| 文件 | 命中数 | 涉及发现 | 维度 | 说明 |
|---|---|---|---|---|
| `src/kiro/token_manager.rs` | 4 | F001/F002/F003/F004 | cor, prf | 凭据生命周期/LB 核心，禁用-恢复-选择三条路径 |
| `src/kiro/provider.rs` | 2 | F001/F002 | cor | F001 余额禁用触发点 + F002 429 冷却触发点 |

## 根因聚类

- **RCG-1（primary F001）** = {F001, F004}：禁用-恢复路径设计缺陷——阈值无滞后 + 指数退避过长 + 全局熔断恢复漏覆盖，共同过度抑制凭据可用性。**建议同批修复**（集中在恢复路径）。
- **RCG-2（primary F002）** = {F002, F003}：`min(recent_usage)` 选择策略对低 usage 值的偏好被滥用——429 冷却凭据无补偿 + 窗口重置阶跃归 1。**F002 重构可顺带覆盖 F003**。

## 优化建议（优先级排序）

1. **P0 — F001**：禁用/恢复阈值解耦 + 滞后区间（禁用用 `remaining < EPS`，恢复用 `>= 1.0`）+ InsufficientBalance 退避封顶 120min → 30min。改动小、风险低，直击用户报告主因。
2. **P1 — F002**：派发计数与成功计数分离，在 `acquire_context` 选中凭据时即记 usage，消除 429 冷却凭据反复撞限流环。
3. **P2 — F004**：`check_and_recover` 到期时按熔断起始快照统一放行（搭车 F001 改恢复路径）。
4. **P3 — F003**：usage 改指数衰减计数（可并入 F002 重构）。
5. **无需动作 — F005**：保持宽松兜底，仅文档化与上游差异。

## 上游同步最终裁定

**结论：无需同步（除 optional 构建链）。**

逐项核实 scanner 的 6 条 upstream_sync 记录：

| 上游 commit | 内容 | current 覆盖情况 |
|---|---|---|
| b9e757e / 4cca715 | Opus 4.7/4.8 + 模型映射 | 已由 a623c8f 独立覆盖，opus 分支更全（4.5/4.6/4.7/4.8） |
| 551b91f | 工具名 >63 字符 | TOOL_NAME_MAX_LEN=63 + shorten_tool_name(SHA256) 全链路覆盖 |
| 2805585 | invalid_grant 立即禁用 | try_ensure_token 已覆盖（mark_authentication_failed） |
| 298505b | accessToken 失效不误禁用 | 已覆盖且更完善（invalidate_access_token + forced_token_refresh 去重） |
| 53df562 | profileArn 动态注入 | 已覆盖（含回归测试） |
| MULTIPLE | musl 静态编译 / 双 CA 信任 | **未覆盖（optional）** |

- current 是 upstream 超集：provider 2323 vs 519 行、token_manager 4181 vs 2599 行；upstream 无 cli endpoint/cooldown/affinity/rate_limiter/background_refresh 模块。
- **唯一 optional**：musl 静态编译链 + 双 CA 信任（current 用纯 rustls-tls）。属构建/部署能力差异，非运行时 bug。若需在企业自签 CA 环境部署，`ff514ba`（双 CA）值得评估；否则跳过。

## 推荐修复范围（交 fixer）

- **必修**：F001（P0）
- **强烈建议**：F002（P1）
- **搭车**：F004 随 F001、F003 随 F002
- **仅文档化**：F005
- **不在本轮**：upstream optional 构建链（交部署需求方定夺）
