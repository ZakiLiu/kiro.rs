# F-007 — Core Module Refactoring

> Role: subject-matter-expert | Related decisions: SA-01, SME-02

## Architecture

The current project uses a monolithic file style for its largest modules: `converter.rs` (~1000+ lines), `stream.rs` (~500+ lines), `token_manager.rs` (~700 lines). The dev-source splits these into sub-module directories (e.g., `converter/` with 6 sub-modules, `token_manager/` with 8 sub-modules).

Per SME-02, the current project's organizational separation (token_manager + cooldown + rate_limiter as separate files) is cleaner than the dev-source's approach of nesting everything under token_manager/. The refactoring strategy MUST preserve this separation while splitting only the largest monolithic files.

Recommended split priorities:
1. `converter.rs` -> `converter/mod.rs` + `converter/model_map.rs` + `converter/tools.rs` + `converter/content.rs` + `converter/history.rs`
2. `stream.rs` -> `stream/mod.rs` + `stream/context.rs` + `stream/sse.rs` + `stream/thinking.rs`
3. `token_manager.rs` — split only if it exceeds 1000 lines; current 700 lines is manageable

The split dimensions follow functional cohesion: model mapping logic, tool conversion, content block processing, and history assembly are each self-contained concerns within the converter.

## Interface Contract

The public API of each module MUST NOT change — the split is purely internal. All existing `pub fn` signatures in `converter.rs` and `stream.rs` remain in `mod.rs` with `pub use` re-exports.

```rust
// converter/mod.rs
mod model_map;
mod tools;
mod content;
mod history;

pub use self::model_map::map_model;
pub use self::tools::{convert_tools, normalize_json_schema};
pub use self::content::convert_content_blocks;
pub use self::history::build_conversation_history;
pub fn convert_request(...) -> ConversationState { ... }
```

Consumers: All existing callers remain unchanged.

## Constraints (RFC 2119)

- The refactoring MUST NOT change any public API signatures — this is a purely internal restructuring.
- The refactoring MUST be preceded by sufficient test coverage on the critical paths (protocol conversion, streaming) to catch regressions (see TS-01).
- The refactoring SHOULD follow the current project's existing module organization style, not the dev-source's style.
- Each sub-module SHOULD be independently testable with unit tests that do not require the parent module's context.
- The refactoring MUST NOT be attempted in the same PR as feature additions — it is a standalone, cross-version effort.

## Test Approach

- Pre-refactor: Ensure all existing tests pass; add tests for any untested public functions.
- During refactor: Run `cargo test` after each file split; verify zero test failures.
- Post-refactor: Run `cargo clippy` to catch dead imports, unused `pub` visibility, and module path issues.

## TODOs

- Count lines and public functions in `converter.rs` and `stream.rs` to confirm split dimensions.
- Identify which internal functions have test coverage and which do not — coverage gaps block the refactoring.
- Create a migration checklist for each file split with rollback instructions.
