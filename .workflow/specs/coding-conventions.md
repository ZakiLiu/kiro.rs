---
title: Coding Conventions
readMode: required
priority: high
category: coding
keywords:
  - style
  - naming
  - import
  - pattern
  - convention
  - formatting
related:
  - "spec:project:architecture-constraints"
  - knowhow-decompose-src-2026-05-24
  - knowhow-follow-provider-2026-05-24
---


# Coding Conventions

Auto-generated from project analysis. Update manually as patterns evolve.

## Formatting
- Indentation: 4 spaces (Rust default)
- Line length: not explicitly configured (Rust default ~100)
- Trailing commas: yes (Rust idiomatic)
- Semicolons: required (Rust)

## Naming
- Variables/functions: snake_case
- Structs/Enums/Traits: PascalCase
- Constants: UPPER_SNAKE_CASE
- Modules/Files: snake_case
- Frontend (TypeScript): camelCase variables, PascalCase components, kebab-case files

## Imports
- Style: explicit `use` declarations
- Order: std → external crates → crate-internal (`crate::`)
- Re-exports: `pub mod` + `pub use` in mod.rs
- Frontend: named imports, `@/` path alias

## Patterns
- Module organization: `mod.rs` as barrel with `pub mod` declarations
- Error handling: `anyhow::Result` for application errors, custom error types for library boundaries
- Concurrency: `Arc<RwLock<T>>` (parking_lot) for shared state, `Arc<Mutex<T>>` for exclusive access
- Async: Tokio runtime, `async fn` handlers
- Feature flags: `#[cfg(feature = "...")]` for conditional compilation
- Documentation: `//!` module-level docs, `///` item docs (Chinese comments)
- Constants: module-level `const` with doc comments explaining purpose
- Builder pattern: struct with `new()` constructor

## Frontend Patterns
- Component library: Radix UI primitives + custom wrappers in `components/ui/`
- Styling: Tailwind CSS v4 + `clsx` + `tailwind-merge`
- State management: TanStack React Query for server state
- HTTP client: Axios

## Entries

<spec-entry category="coding" keywords="failover,故障转移,凭据,重试,exclusion" date="2026-05-24">
### 多凭据故障转移链 (Credential Failover Chain)
请求失败时通过 exclusion list 排除已失败凭据，acquire_context_excluding 选择下一个可用凭据。网络错误不 push failed_ids（Round 11 决议：链路问题与凭据无关）。402 额度用尽时 disable + push。所有凭据 disabled 时触发 auto-heal 重置。参考实现：`src/kiro/provider.rs:596-1000`。
</spec-entry>

<spec-entry category="coding" keywords="backoff,jitter,重试,指数退避,retry" date="2026-05-24">
### 指数退避 + 抖动 (Exponential Backoff + Jitter)
重试延迟公式：`BASE_MS * 2^attempt`，cap at MAX_MS (2000ms)，加 25% 随机 jitter（fastrand）。防止 thundering herd。总重试硬上限 MAX_TOTAL_RETRIES=3。Retry-After header 解析支持 integer seconds 和 RFC 2822 datetime，clamp 到 [60s, 300s]。参考实现：`src/kiro/provider.rs:984-993`。
</spec-entry>

<spec-entry category="coding" keywords="hot-update,热更新,RwLock,运行时配置" date="2026-05-24">
### 运行时热更新模式 (Hot-Update Pattern)
可变配置使用 RwLock 包装，通过 update_* 方法原子替换。热更新全局代理时同步重建 default_client 并清空 client_cache。热更新 endpoint 时验证注册表包含目标名称。模式：读多写少 → RwLock；写时验证 → assert/Result；副作用清理 → 同步执行。参考实现：`src/kiro/provider.rs:115-139`。
</spec-entry>

