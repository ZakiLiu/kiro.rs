---
title: Architecture Constraints
readMode: required
priority: high
category: arch
keywords:
  - architecture
  - module
  - layer
  - boundary
  - dependency
  - structure
related:
  - "spec:project:coding-conventions"
  - knowhow-decompose-src-2026-05-24
  - knowhow-follow-provider-2026-05-24
---


# Architecture Constraints

Auto-generated from project structure. Update manually as architecture evolves.

## Module Structure
- Type: single-package (Rust binary + embedded frontend)
- Key modules:
  - `src/anthropic/` — Anthropic API 兼容层（路由、handler、协议转换、流式响应）
  - `src/kiro/` — Kiro API 客户端（provider、token 管理、事件解析、凭据管理）
  - `src/admin/` — Admin API（凭据 CRUD、状态监控）
  - `src/admin_ui/` — 前端静态文件服务（rust-embed）
  - `src/common/` — 共享工具（认证、脱敏、UTF-8 处理）
  - `src/model/` — 全局模型（CLI 参数、配置）
  - `admin-ui/` — React 前端（独立 Vite 项目）

## Layer Boundaries
```
HTTP Request → anthropic/ (Axum handlers)
  → common/auth (认证中间件)
  → anthropic/converter (协议转换)
  → kiro/provider (API 调用 + 重试 + 故障转移)
    → kiro/token_manager (凭据管理 + Token 刷新)
    → kiro/parser (Event Stream 解析)
  → anthropic/stream (SSE 响应转换)
```

## Dependency Rules
- `anthropic/` → `kiro/`, `common/`, `model/`
- `kiro/` → `common/`, `model/` (不依赖 anthropic/)
- `admin/` → `kiro/`, `common/`, `model/`
- `common/` → 无内部依赖（纯工具层）
- `model/` → 无内部依赖（纯数据定义）

## Technology Constraints
- Runtime: Rust 2024 edition, Tokio async runtime
- Module system: Rust module system (mod.rs pattern)
- TLS: rustls (no OpenSSL dependency)
- Frontend build: must complete before backend compile (rust-embed)
- Single binary deployment: frontend embedded via `#[derive(Embed)]`

## Key Architectural Decisions
- `serde_json` with `preserve_order` feature — Kiro CLI wire byte-alignment requirement
- `parking_lot` over std sync — better performance, no poisoning
- `subtle` for API key comparison — constant-time to prevent timing attacks
- Per-credential HTTP client caching — avoid recreating clients for proxy configs

## Entries



<spec-entry category="arch" keywords="token,估算,metering,cache,kiro-protocol" date="2026-06-06" source="analyze-output-token-growth">

### Kiro 上游无 token 计数 — 全量本地估算

Kiro Event Stream 不返回任何 token 数（input/output/thinking/cache 全部没有）。只返回 MeteringEvent.usage（credit 消耗 float）和 ContextUsageEvent.contextUsagePercentage（上下文窗口百分比）。proxy 通过 estimate_tokens() 启发式估算所有 token 数，CacheTracker 完全模拟缓存命中。参考：src/kiro/model/events/（6 种事件类型）、src/anthropic/stream/usage.rs（估算函数）、src/anthropic/cache_tracker.rs（缓存模拟）。

</spec-entry>

<spec-entry category="arch" keywords="thinking,output,token,分离,估算" date="2026-06-06" source="quick-thinking-tokens-separation">

### Output/Thinking tokens 分离计数

output_tokens 不含 thinking 内容。thinking tokens 通过 StreamContext.thinking_tokens 独立累计（从 reasoningContentEvent 估算），在 message_delta usage 中以 thinking_tokens 字段单独报告（仅 >0 时输出）。修复于 2026-06-06：stream/context.rs:295 从 output_tokens 移到 thinking_tokens。参考：src/anthropic/stream/context.rs、src/anthropic/stream/usage.rs。

</spec-entry>

<spec-entry category="arch" keywords="token,tokenusageevent,精确,计量,cache,billing" date="2026-06-06" source="analyze-kiro-cli-debug">

### tokenUsageEvent 上游精确 token 计量

Kiro 后端在流末端下发 tokenUsageEvent 事件，包含精确的 uncachedInputTokens、outputTokens(含thinking)、totalTokens、cacheReadInputTokens、cacheWriteInputTokens。proxy 通过 BillingSplit 转换为 Anthropic 三段不重叠计费口径（fresh 1×、cache_read 0.1×、cache_write 1.25×）。有 tokenUsageEvent 时覆盖本地估算；无此事件时回退到 estimate_tokens 启发式。参考：src/kiro/model/events/token_usage.rs、src/anthropic/stream/context.rs generate_final_events。

</spec-entry>