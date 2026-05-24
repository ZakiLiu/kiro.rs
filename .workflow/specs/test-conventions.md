---
title: Test Conventions
readMode: required
priority: high
category: test
keywords:
  - test
  - coverage
  - mock
  - fixture
  - assertion
  - framework
related:
  - "spec:project:coding-conventions"
---


# Test Conventions

Auto-generated from project analysis. Update manually as patterns evolve.

## Framework
- Framework: Rust built-in `#[cfg(test)]` + `#[test]` macros
- Run command: `cargo test`
- No external test framework (no mockall, no proptest detected)

## Directory Structure
- Pattern: co-located `#[cfg(test)] mod tests {}` blocks within source files
- Standalone test file: `src/test.rs` (integration/manual testing helper)
- 32 source files contain test modules

## Naming Conventions
- Test functions: `#[test] fn test_<description>()`
- Test modules: `mod tests` (standard Rust convention)

## Patterns
- Unit tests co-located with implementation
- `assert!`, `assert_eq!`, `assert_ne!` for assertions
- No mocking framework — tests use real implementations
- Frontend: no test framework configured (no vitest/jest in package.json)

## Entries

