---
related:
  - "spec:project:coding-conventions"
  - knowhow-decompose-src-2026-05-24
  - "spec:project:architecture-constraints"
  - knowhow-periodic-recovery-2026-05-25
---

# Understanding Map: src/kiro/provider.rs
**Generated:** 2026-05-24 | **Lines:** 2185 | **Depth:** shallow (W003: >1000 lines)

## Key Concepts

`KiroProvider` 是整个系统的韧性核心——它不只是一个 HTTP 客户端封装，而是一个**多凭据、多端点、自适应容错的请求路由器**。

### 职责分层

```
KiroProvider
├── 构造 & 配置热更新 (lines 50-181)
│   ├── Endpoint 注册表 (HashMap<String, Arc<dyn KiroEndpoint>>)
│   ├── Client 缓存 (per-credential proxy → cached reqwest::Client)
│   └── 热更新: update_global_proxy / update_default_endpoint
├── 后台刷新 (lines 196-267)
│   ├── start_periodic_balance_refresh (ticker, 串行 + 300ms 间隔)
│   └── spawn_balance_refresh (fire-and-forget, post-success 触发)
├── 请求执行 (lines 269-1000)
│   ├── call_mcp (MCP 请求, 简化重试)
│   └── call_api_with_retry (核心! 多凭据故障转移 + 分类重试)
├── 辅助函数 (lines 984-1268)
│   ├── retry_delay (exponential + 25% jitter)
│   ├── parse_retry_after (seconds + RFC2822, clamp [60s, 300s])
│   ├── handle_rate_limited_response (设置 cooldown)
│   ├── is_* 谓词 (字符串匹配错误分类)
│   ├── estimate_tokens (CJK/Latin 启发式)
│   └── wire_capture (debug-only, OnceLock 缓存 env var)
└── 测试 (lines 1271-2185)
    └── 26 个单元测试, 含 Round 4/8/11 回归守卫
```

## Patterns (with convention cross-reference)

| # | Pattern | Location | Convention Status |
|---|---------|----------|-------------------|
| 1 | Composition + Arc wrapping | :54-66 | documented |
| 2 | Strategy (KiroEndpoint trait objects) | :62-63, :174-181 | **new** — candidate for spec-add |
| 3 | Client cache with lazy creation + fallback | :144-172 | **new** |
| 4 | Hot-update via RwLock write | :115-139 | **new** — runtime reconfiguration pattern |
| 5 | Dual background refresh (periodic + lazy) | :196-267 | **new** |
| 6 | Chain of Responsibility (failover loop) | :596-1000 | **new** — candidate for spec-add |
| 7 | Exponential backoff + jitter | :984-993 | **new** — candidate for spec-add |
| 8 | Error classification cascade (string-based) | :1054-1116 | **new** — known tech debt |
| 9 | Retry-After parsing + clamping | :1026-1049 | **new** |
| 10 | Wire capture (debug-only, OnceLock) | :1220-1238 | **new** — debug tooling pattern |
| 11 | Regression test guards (Round N) | :1508+ | **new** — testing discipline |

## Assumptions (Implicit Contracts)

| # | Assumption | Evidence | Risk if violated |
|---|-----------|----------|-----------------|
| 1 | 400 errors are never transient | Line 770: immediate bail | Request fails without retry on transient 400 |
| 2 | Network errors are credential-agnostic | Line 706: "Round 11 修订" | If one credential's network path is broken, retries same credential |
| 3 | Retry-After ∈ [60s, 300s] is reasonable | Line 1044-1048: clamp | Too-early retry if upstream legitimately needs >300s |
| 4 | Client cache key = credential_id (not proxy hash) | Line 154 | Stale client if proxy config changes without cache clear |
| 5 | Endpoint registry is immutable after construction | No RwLock on endpoints | Cannot add endpoints at runtime |
| 6 | MAX_TOTAL_RETRIES=3 is sufficient | Line 42 | May not try all credentials in large pools |
| 7 | 300ms inter-credential sleep prevents upstream rate limiting | Line 238 | Insufficient for aggressive rate limiters |
| 8 | estimate_tokens is for logging only | Line 1186+ | Would be inaccurate for billing |

## Decision Archaeology (Round Comments)

| Round | Decision | Location | Impact |
|-------|----------|----------|--------|
| P0#1 | 402 凭据 push to failed_ids 防竞争窗口 | :765 | Prevents re-selection of exhausted credential |
| P0#3 | 周期性 balance 刷新 (was frozen snapshot) | :189-192 | LB gets accurate balance signals |
| Round 4 | 不重写 user tool inputSchema 中的 origin/modelId | :1508 | Regression guard |
| Round 4 | 每请求注入 credential profileArn (was static) | :1575 | Multi-credential rotation fix |
| Round 4 | IDC 凭据 strip profileArn | :1605 | Auth method compatibility |
| Round 8 | is_rate_limit_response 默认不接入 | :1051 | Dead code, decision preserved |
| Round 11 | 网络错误不 push failed_ids | :706-708 | Prevents mass credential disable on network flap |

## Open Questions

1. **client_cache 无驱逐策略** — 如果凭据被删除，对应的 cached client 永远不会被清理。是否需要 LRU 或 TTL？
2. **is_rate_limit_response 是 dead code** — Round 8 决议后未接入，但代码和测试都保留。是否应该清理或重新接入？
3. **MAX_TOTAL_RETRIES=3 vs 大凭据池** — 如果有 10 个凭据，只尝试 3 个就放弃。是否应该动态调整？
4. **字符串错误分类的脆弱性** — 上游消息格式变更会静默破坏分类。是否应该迁移到 typed error enum？
5. **try_resume() 在 decoder 中是 dead code** — Stopped 状态在生产中不可恢复。是否需要实现恢复路径？

## Connections

```
provider.rs
├── depends on → token_manager.rs (Arc<MultiTokenManager>)
├── depends on → endpoint/mod.rs (KiroEndpoint trait)
├── depends on → cooldown.rs (CooldownReason)
├── depends on → http_client.rs (build_client, ProxyConfig)
├── depends on → machine_id.rs (get_machine_id)
├── used by → anthropic/handlers.rs (call_api_with_retry)
├── used by → admin/service.rs (via token_manager)
└── used by → main.rs (construction + start_periodic_balance_refresh)
```

## Resilience Architecture Summary

```
Request arrives
  │
  ▼
acquire_context_excluding(user_id, failed_ids)
  │ ← affinity-based selection, skip cooldown/disabled/excluded
  │
  ▼
endpoint_for(credentials) → Arc<dyn KiroEndpoint>
  │
  ▼
get_client_for_credential → cached or create
  │
  ▼
endpoint.transform_api_body + decorate_api
  │
  ▼
request.send().await
  │
  ├── Ok(2xx) → report_success + spawn_balance_refresh → return
  │
  ├── Ok(400) → bail immediately (not retryable)
  │
  ├── Ok(401/403) → report_failure + push failed_ids → retry next credential
  │
  ├── Ok(402 + MONTHLY_REQUEST_COUNT) → report_quota_exhausted + push → retry
  │
  ├── Ok(429) → handle_rate_limited_response (set cooldown) + push → retry
  │
  ├── Ok(5xx + MODEL_TEMPORARILY_UNAVAILABLE) → report_model_unavailable → retry
  │
  ├── Ok(5xx other) → report_failure + push → retry
  │
  └── Err(network) → DON'T push failed_ids → retry same credential after backoff
```
