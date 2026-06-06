# F-007 — Core Module Refactoring

> Role: product-manager | Related decisions: PM-04, SA-01

## Architecture

The current project has several monolithic modules that exceed maintainability thresholds:
- converter.rs (~1000+ lines) — handles all protocol conversion in one file
- stream.rs (~500+ lines) — handles all event stream processing in one file
- token_manager.rs (~700 lines) — handles all credential management in one file

Dev-source demonstrates effective modularization: converter/ has 6 sub-modules, stream/ has 6 sub-modules, token_manager/ has 8 sub-modules. Each sub-module has clear responsibility boundaries and can be tested independently.

Product perspective: Module refactoring does not deliver direct user value, but it is a force multiplier for all other features. Adding cache, metrics, and error mapping into a monolithic codebase creates compounding complexity. Refactoring SHOULD proceed incrementally alongside feature development, not as a blocking prerequisite.

This is a cross-version concern — it will span multiple releases and should be driven by SA priorities.

## Interface Contract

- **converter.rs** splits into: model_map, session, tools, content, history sub-modules
- **stream.rs** splits into: signature, thinking, sse_state, context, buffered sub-modules
- **token_manager.rs** splits into: acquire, admin_ops, failure, persistence, refresh, selection sub-modules
- **Public API**: Module split MUST NOT change any public API surface; all consumers MUST continue to import from the parent module

## Constraints (RFC 2119)

- MUST NOT change public API during refactoring (consumers import from mod.rs re-exports)
- MUST maintain backward compatibility with all existing tests
- MUST refactor incrementally — one module per release, not a big-bang restructure
- SHOULD prioritize converter.rs split first (largest file, most active development surface for F-003 and F-004)
- SHOULD align refactoring with feature development (split converter when implementing F-004)
- MAY defer token_manager refactoring if the current structure remains manageable

## Test Approach

- Before refactoring: ensure existing test coverage for the module being split (add tests if missing per TS-01)
- After refactoring: all existing tests MUST pass without modification
- No new functionality introduced during refactoring — purely structural change

## TODOs

- Measure current test coverage for converter.rs, stream.rs, token_manager.rs
- Define refactoring order based on feature development needs (likely: converter first for F-004)
- Establish naming conventions for sub-modules (follow dev-source patterns or project conventions)
- Coordinate with SA and TS roles on refactoring approach and timing
