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

