# Analysis: Output Token Growth Problem

## §1. Problem Statement

用户观察到 Claude Code 使用 kiro.rs 代理时，output token (↑ 绿色箭头) 持续增长，担心"没有缓存到"导致计费高。

## §2. Screenshot Data Analysis

| 指标 | 观察值 | 趋势 | 判断 |
|------|--------|------|------|
| ↓ input | 543K→544K | 缓慢增长 | 正常：对话越长输入越多 |
| 📦 cache_read | 187K→190K | 缓慢增长 | 正常：缓存在工作 |
| ↑ output | 18→392 (波动) | 非单调增长 | 每轮输出不同是正常的 |
| ✏️ cache_creation | 46→638 (波动) | 波动大 | 每次有新前缀被创建 |

## §3. Root Cause Analysis

### 核心结论：Output Token 增长是 **正常行为**，不是 bug

1. **Output 永远不能被缓存** — Anthropic API 只缓存 input（prompt prefix），output（模型生成内容）每次都是新生成的。这是 API 设计决定的，不是 kiro.rs 的问题。

2. **Output token 是本地估算值** — Kiro 上游只返回 `MeteringEvent.usage`（credit 消耗），不返回 output token 数。proxy 通过 `estimate_tokens()` 粗糙估算（CJK ~0.67 tok/char, ASCII ~0.25 tok/char）。

3. **每轮对话自然产生新 output** — 模型每次回复都生成新的 output token，累积总量自然增长。

### 证据链

| 文件 | 行号 | 发现 |
|------|------|------|
| `stream/context.rs` | 376 | `self.output_tokens += estimate_tokens(content)` — 每个 chunk 累加本地估算 |
| `stream/context.rs` | 295 | thinking block 也被计入 output_tokens |
| `stream/usage.rs` | 34-52 | `estimate_tokens()` 粗糙启发式估算 |
| `stream/state.rs` | 267 | `"output_tokens": usage.output_tokens` — 报告给客户端的是估算值 |
| `kiro/model/events/metering.rs` | 17-27 | MeteringEvent 只有 `usage: f64`（credit），无 output token 数 |
| `kiro/model/events/context_usage.rs` | 17-21 | ContextUsageEvent 只有百分比，无 token 数 |

### 不可修复项

- Output token 不可缓存 — API 层面不支持
- 每轮对话生成新 output — 模型本质行为
- 上游不提供 output token 精确值 — Kiro Event Stream 设计限制

### 可优化项

1. **estimate_tokens() 精度** — 当前估算可能偏高或偏低，影响用户对成本的判断
2. **Thinking block 被计入 output_tokens** — `context.rs:295` 把 reasoning text 也累加进了 output_tokens，但 Anthropic API 规范中 thinking tokens 应该单独计数，不计入 output_tokens
3. **Input 侧缓存优化** — 虽然不能减少 output，但可以通过提高 cache hit rate 降低总成本
4. **利用 MeteringEvent.usage 交叉验证** — 可以基于 credit 消耗反推真实 output token 数

## §4. Recommendations

### R-001: 修复 thinking tokens 计入 output_tokens 的问题 (HIGH)

**现状**：`stream/context.rs:295` 把 reasoning text 也加到 `self.output_tokens`，这导致：
- 用户看到的 output_tokens 包含了 thinking content
- Anthropic API 规范要求 thinking tokens 不计入 output_tokens
- 这会导致用户觉得 output "虚高"

**修复**：用独立计数器 `self.thinking_tokens` 跟踪 reasoning content，不加到 `output_tokens` 里。

### R-002: 利用 MeteringEvent 交叉验证 output 估算 (MEDIUM)

**现状**：MeteringEvent.usage 包含 credit 消耗（含 output token 成本），但 proxy 完全忽略了这个信号。

**修复**：在 sensitive-logs 模式下，输出 MeteringEvent.usage 与本地估算的比较日志，帮助用户判断估算准确度。

### R-003: 提高 input 侧缓存命中率 (MEDIUM)

**现状**：cache_read 约 189K / input 544K ≈ 35% 命中率偏低。

**方向**：检查 conversation_id 复用是否生效（CrossRequestCache 已实现），确认 cache_control breakpoint 位置是否最优。

## §5. Non-Goals

- 缓存 output token — 技术上不可能
- 减少模型生成量 — 不在 proxy 控制范围内
- 修改上游 Kiro 协议 — 不可控
