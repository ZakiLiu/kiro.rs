# F-007 — Core Module Refactoring

> Role: test-strategist | Related decisions: TS-09

## Architecture

Module refactoring (splitting converter.rs, stream.rs, token_manager.rs into sub-modules) is a structural change that MUST NOT alter behavior. The existing 378 tests across 33 files serve as the primary regression gate. The test strategy is purely defensive: no new functionality is introduced, so no new tests are needed — the gate is "all existing tests pass."

Key testable aspects:
- **Zero regression** — Every existing test MUST pass before and after each module split.
- **Public API stability** — Module refactoring MUST NOT change public function signatures.
- **Import path changes** — Internal re-exports MUST maintain backward compatibility.

Dev-source demonstrates a modular layout (converter/ with 6 sub-modules, stream/ with 6 sub-modules) that serves as a structural reference (see SA-01).

## Interface Contract

No new interfaces. The refactoring MUST preserve all existing interfaces:
- `converter::convert_request()` signature unchanged.
- `stream::StreamContext` API unchanged.
- `TokenManager` public methods unchanged.

The test suite itself is the contract — if tests pass, the refactoring is correct.

## Constraints (RFC 2119)

- Module splitting MUST NOT change any public API signature.
- All 378 existing tests MUST pass after each split operation (not just after the final split).
- The `make check` pipeline (fmt + clippy + test) MUST pass at every intermediate commit.
- Each module split SHOULD be a single atomic commit to enable bisection if regression occurs.
- New sub-modules SHOULD re-export types via `mod.rs` to maintain import compatibility.

## Test Approach

**Regression-only strategy:**
1. Run `cargo test` before starting refactor — record pass count (expected: 378).
2. After each module split, run `cargo test` — verify identical pass count.
3. Run `cargo clippy` — verify no new warnings introduced.
4. Run `cargo fmt --check` — verify formatting compliance.

**Incremental verification:**
- Split converter.rs first (56 tests, highest test density).
- Split stream.rs second (37 tests).
- Split token_manager.rs third (30 tests).
- After each split, verify the full suite, not just the affected module.

**No new tests needed** — The refactoring does not introduce new behavior. Adding tests during refactoring would conflate structural and behavioral changes.

## TODOs

- Record exact baseline test count and per-module counts before starting.
- Plan the split order: largest module first or most-tested module first.
- Verify that `#[cfg(test)]` modules within split files are correctly distributed to sub-modules.
