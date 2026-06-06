# TASK-003: Split converter.rs, stream.rs, token_manager.rs into sub-module directories

## Changes
- `src/anthropic/converter.rs` → `src/anthropic/converter/` directory with mod.rs + 6 sub-modules (model.rs, schema.rs, tools.rs, history.rs, content.rs, system.rs)
- `src/anthropic/stream.rs` → `src/anthropic/stream/` directory with mod.rs + 5 sub-modules (event.rs, state.rs, thinking.rs, context.rs, usage.rs)
- `src/kiro/token_manager.rs` → `src/kiro/token_manager/` directory with mod.rs + 5 sub-modules (types.rs, single.rs, refresh.rs, balance.rs, multi.rs)
- No changes to `src/anthropic/mod.rs`, `src/kiro/mod.rs`, or `src/anthropic/handlers.rs` — Rust module resolution automatically finds directory+mod.rs

## Verification
- [x] converter/ directory exists with mod.rs + 6 sub-modules: verified via `ls`
- [x] stream/ directory exists with mod.rs + 5 sub-modules: verified via `ls`
- [x] token_manager/ directory exists with mod.rs + 5 sub-modules: verified via `ls`
- [x] All imports compile without changes: `cargo test` passes, no external import path changes needed
- [x] `cargo test` produces identical results: 436 passed, 0 failed (same count before and after each split)
- [x] `cargo clippy -- -D warnings` clean: no warnings after each split
- [x] No changes to handlers.rs: verified via `git diff --name-only`
- [x] No changes to any file outside the 3 split modules: verified via `git diff --name-only`

## Tests
- [x] `cargo test`: 436 passed, 0 failed, 0 ignored — identical to baseline
- [x] `cargo clippy -- -D warnings`: clean, no warnings

## Deviations
- token_manager split: some re-exports removed from mod.rs that were unused externally (TokenManager, CredentialEntrySnapshot, ManagerSnapshot, refresh_token, refresh_token_with_id, is_token_*, validate_*). These items are still accessible via the `pub(crate)` sub-modules but not re-exported at the `token_manager` module level since no external code imports them by that path.
- SseStateManager: changed from `pub` re-export to accessible only through the stream sub-module (no external code imports it by name; accessed indirectly via StreamContext.state_manager field)
- Private helper functions made `pub(super)` where needed for cross-sub-module access (e.g., balance constants, thinking tag functions, tool name helpers)

## Notes
- All 3 splits follow the existing `src/kiro/parser/` sub-module pattern
- Each split was committed independently with `cargo test` green before proceeding to the next
- The converter orchestrator function `convert_request()` remains in mod.rs; all tests remain in mod.rs for each split
