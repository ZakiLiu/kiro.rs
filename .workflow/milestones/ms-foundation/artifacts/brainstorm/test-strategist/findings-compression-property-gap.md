# Finding: Compression Pipeline Lacks Property-Based Testing

> Role: test-strategist | Impact: MEDIUM

## Description

The compression pipeline (compressor.rs, 33 tests) is a multi-stage transformation: whitespace compression, thinking truncation, tool_result truncation, tool_use input truncation, and history truncation. Each stage has well-defined input/output contracts and size invariants. This makes the pipeline an ideal candidate for property-based testing via `proptest`, yet all 33 existing tests are example-based.

Property-based testing would catch edge cases that example-based tests miss:
- Input ordering invariants (compression MUST be idempotent or monotonically size-reducing).
- Tool use/tool result pairing preservation across truncation.
- Unicode boundary safety during whitespace compression.
- Size bound guarantees (output MUST be ≤ input in bytes).

## Affected Features

- Compression pipeline (existing, SA-06 MUST NOT remove).
- F-003 (error mapping) — Compression-induced upstream errors need to be mapped correctly.
- F-007 (module refactor) — If compressor.rs is refactored, property tests provide stronger regression guarantees than example tests alone.

## Recommendation

Add `proptest` as a dev-dependency and write 3-5 property tests for the compression pipeline:
1. Output size ≤ input size for any valid MessagesRequest.
2. Compressed output is valid JSON (no mid-character truncation).
3. Tool use/result pairing is preserved after compression.
4. Compression is deterministic (same input always produces same output).
5. Repeated compression produces same result (idempotency or convergence).

This investment pays off immediately for the existing codebase and amplifies during F-007 refactoring.
