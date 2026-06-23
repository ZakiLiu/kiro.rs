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
  - knowhow-knw-decompose-src-2026-05-24
  - knowhow-knw-follow-provider-2026-05-24
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



<spec-entry category="coding" keywords="estimate,token,精度,启发式,metering" date="2026-06-06" source="analyze-output-token-growth">

### estimate_tokens 启发式精度限制

estimate_tokens() 使用简单字符分类估算（CJK ~0.67 tok/char, ASCII ~0.25 tok/char），无法对照真实值验证精度。input_tokens 有 contextUsagePercentage 交叉校准（context.rs:182），output/thinking 完全依赖估算。改进方向：可利用 MeteringEvent.usage(credit) 反推总 token 做一致性检查，但需要 credit→token 换算公式。参考：src/anthropic/stream/usage.rs:34-52。

</spec-entry>

<spec-entry category="coding" keywords="429,阈值,backoff,限流,重试策略,总计数器" date="2026-06-23" source="harvest:analyze-kiro-429-500-root-cause">

### 429 重试策略：backoff + 固定阈值 + 总计数器

429 处理三件套：(1) 429 路径添加 backoff（与 5xx 一致 `sleep(retry_delay(attempt))`）；(2) 连续 429 阈值固定为 3（3 个不同凭据连续返回 429 = 全局限流的确证）；(3) 新增不重置的 total_429_count，阈值 5（捕获 500/网络错误与 429 交替的边缘场景）。consecutive 计数沿用现有逻辑，total 计数跨凭据累计不重置。参考来源：.workflow/scratch/20260609-analyze-kiro-429-500-root-cause/。

</spec-entry>

<spec-entry category="coding" keywords="pdf,lopdf,feature-gated,提取,优雅降级,10MB" date="2026-06-23" source="harvest:plan-P1-features-refactoring">

### PDF 提取 feature-gated + 优雅降级

lopdf 依赖通过 Cargo feature `pdf-support` 门控（非默认编译）。PDF 处理在 convert_request 协议转换前执行，将 document 块替换为 text 块。提取失败必须降级为占位文本，不阻断请求。限制：原始 PDF 最大 10MB，提取文本最大 200K 字符。参考来源：.workflow/scratch/20260605-plan-P1-features-refactoring/。

</spec-entry>

<spec-entry category="coding" keywords="重构,mod.rs,re-export,机械化,零功能变更,commit" date="2026-06-23" source="harvest:plan-P1-features-refactoring">

### 模块拆分机械化：mod.rs re-export 保证向后兼容

大文件拆分为子模块：代码移入子模块文件，mod.rs 用 `pub mod` + `pub use` re-export 保持对外接口不变（下游 import 零修改）。约束：每次拆分一个 commit，cargo test 结果必须前后一致（零功能变更）。converter.rs → 6 子模块、stream.rs → 5 子模块、token_manager.rs → 5 子模块。参考来源：.workflow/scratch/20260605-plan-P1-features-refactoring/。

</spec-entry>

<spec-entry category="coding" keywords="preset,system_prompt,x-preset-id,注入顺序,压缩,config" date="2026-06-23" source="harvest:plan-P1-features-refactoring">

### Prompt Preset 机制：x-preset-id 选择 + 压缩前注入

Preset（可配置 system prompt 库）通过请求头 `x-preset-id` 选择，不用 prompt filter（安全审查后延期）。注入顺序：preset system_prompt 在压缩管道**之前** prepend，确保压缩后的请求包含 preset 内容。存储于 config.json 的 `presets` 数组字段。Admin UI 延期，先实现 API-only。参考来源：.workflow/scratch/20260605-plan-P1-features-refactoring/。

</spec-entry>

<spec-entry category="coding" keywords="contextUsageEvent,缓冲,input_tokens,精确计算,StreamContext,buffering" date="2026-06-23" source="harvest:plan-port-ops-frontend">

### contextUsageEvent 缓冲模式实现精确 input_tokens

StreamContext 增加 buffering mode flag，缓存 message_start 事件直到 contextUsageEvent 到达，用其 contextUsagePercentage 精确计算 input_tokens（取代纯启发式估算）。参考来源：.workflow/scratch/20260615-plan-port-ops-frontend/。

</spec-entry>

<spec-entry category="coding" keywords="cli.rs,endpoint,不可修改,防检测,只读约束,ide.rs" date="2026-06-23" source="harvest:analyze-P1-features-refactoring">

### CLI endpoint 文件为只读约束

kiro/endpoint/cli.rs 和 kiro/endpoint/ide.rs 在任何重构中不得修改——CLI endpoint 对反检测至关重要，是项目硬约束。参考来源：.workflow/scratch/20260605-analyze-P1-features-refactoring/。

</spec-entry>