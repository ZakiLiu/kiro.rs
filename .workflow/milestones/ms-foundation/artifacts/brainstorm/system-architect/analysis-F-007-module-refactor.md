# F-007 -- Core Module Refactoring

> Role: system-architect | Related decisions: SA-01, TS-02

## Architecture

Three monolithic source files are candidates for sub-module decomposition:

### converter.rs (~1000 LOC) -> converter/

Split into sub-modules mirroring dev-source dimensions but preserving current project conventions:

| Sub-module | Responsibility | Approx LOC |
|-----------|---------------|-----------|
| converter/mod.rs | Public API, convert_request() orchestration | ~150 |
| converter/model_map.rs | Model name mapping (sonnet/opus/haiku -> Kiro IDs) | ~100 |
| converter/tools.rs | Tool definition conversion, schema normalization, name shortening (F-004) | ~200 |
| converter/content.rs | Content block processing (text, image, document/PDF) | ~250 |
| converter/session.rs | Session/conversation management, conversation_id injection (F-004) | ~100 |
| converter/history.rs | Message history processing, prefill handling | ~200 |

### stream.rs (~500 LOC) -> stream/

| Sub-module | Responsibility | Approx LOC |
|-----------|---------------|-----------|
| stream/mod.rs | Public API, StreamContext creation | ~100 |
| stream/context.rs | StreamContext state machine, event accumulation | ~150 |
| stream/sse.rs | SSE formatting, event serialization | ~100 |
| stream/thinking.rs | Thinking block handling | ~80 |
| stream/signature.rs | Signature/buffered event processing | ~70 |

### token_manager.rs (~700 LOC) -> token_manager/

The current project already has clean separation (token_manager + cooldown + rate_limiter + affinity + background_refresh), which is better than dev-source grouping everything under token_manager/. Per SA-05, the current separation MUST be preserved. However, token_manager.rs itself can benefit from internal decomposition:

| Sub-module | Responsibility | Approx LOC |
|-----------|---------------|-----------|
| token_manager/mod.rs | MultiTokenManager public API | ~150 |
| token_manager/selection.rs | Credential selection algorithm (priority + affinity + balance) | ~200 |
| token_manager/refresh.rs | Token refresh logic (Social + IdC auth methods) | ~200 |
| token_manager/persistence.rs | Credential file read/write | ~100 |
| token_manager/admin_ops.rs | Admin API operations (disable, reset, priority) | ~50 |

**Refactoring strategy:** Per SA-01, the refactoring SHOULD proceed incrementally:
1. Phase 1 (with P0 features): Split converter.rs to accommodate F-004 enhancements
2. Phase 2 (with P1 features): Split stream.rs to accommodate error_map integration
3. Phase 3 (maintenance): Split token_manager.rs for cleanliness

Each split MUST be a standalone commit with no functional changes -- pure structural refactor verified by `cargo test` passing.

- Module splits MUST maintain ≥ 80% line coverage on new sub-modules per TS coverage gate (TS-09, TS Coverage Gate interface).

## Interface Contract

The refactoring MUST NOT change any public API. All existing `pub` functions and types retain their signatures. The `mod.rs` files re-export everything that was previously public.

```rust
// converter/mod.rs re-exports
pub use content::process_content_blocks;
pub use model_map::map_model_name;
pub use tools::convert_tools;
pub fn convert_request(...) -> ... { /* orchestrates sub-modules */ }

// stream/mod.rs re-exports  
pub use context::StreamContext;
pub fn create_stream_response(...) -> ... { /* orchestrates sub-modules */ }
```

## Constraints (RFC 2119)

- MUST NOT change any public API surface during refactoring
- MUST NOT introduce functional changes -- each refactor commit is pure structural
- MUST pass `cargo test` after each split commit
- SHOULD split files exceeding 500 LOC
- SHOULD preserve current module visibility (pub vs pub(crate) vs private)
- MUST NOT refactor modules that are not in the target list (avoid scope creep)
- SHOULD coordinate timing with feature implementation to minimize merge conflicts

## Test Approach

- **Verification:** `cargo test` passes before and after each split. `cargo clippy` produces no new warnings.
- **API compatibility:** Grep for all import paths referencing the split module; verify all still resolve.
- **Build verification:** Both `cargo build` and `cargo build --release` succeed.
- **No behavioral tests needed** since refactoring introduces no functional changes.

## TODOs

- Count exact LOC for each target file to confirm split candidates
- Identify all cross-module references to converter.rs, stream.rs, token_manager.rs
- Determine if any macros or conditional compilation (cfg features) complicate the split
- Coordinate with F-004 and F-001 feature branches to minimize merge conflicts
