# F-004 — Converter Layer Enhancements

> Role: test-strategist | Related decisions: TS-01, TS-06

## Architecture

The converter already has the strongest test suite in the project (56 tests in converter.rs). Enhancements (tool name shortening, forced conversation_id) MUST be tested with the same rigor. Tests SHOULD be added to the existing `#[cfg(test)]` module in converter.rs or a new sub-module if the file is split (see F-007).

Key testable components:
- **Tool name shortening** — Automatic abbreviation of tool names exceeding a length threshold, with a reversible mapping table.
- **Forced conversation_id** — Injection of conversation_id into outgoing Kiro requests to improve cache hit rates.
- **Round-trip fidelity** — Shortened tool names in the request MUST be expanded back to originals in the response.
- **Collision avoidance** — Shortening algorithm MUST NOT produce collisions for different input names.

## Interface Contract

- `shorten_tool_name(name: &str, max_len: usize) -> (String, Option<ShortenedMapping>)` — Returns shortened name and optional mapping if shortening occurred.
- `expand_tool_name(short_name: &str, mappings: &[ShortenedMapping]) -> String` — Restores original name from mapping table.
- `inject_conversation_id(request: &mut KiroRequest, conversation_id: &str)` — Mutates the request in place.

## Constraints (RFC 2119)

- Tool name shortening MUST be reversible — `expand(shorten(name)) == name` for all inputs.
- Shortening MUST NOT produce collisions for any two distinct tool names within a single request.
- Names under the threshold MUST NOT be modified.
- Forced conversation_id injection MUST NOT overwrite an explicitly user-provided conversation_id.
- All existing 56 converter tests MUST continue to pass after enhancement.

## Test Approach

**Unit tests (≥ 12 tests):**
1. Name under threshold — no shortening occurs.
2. Name at threshold boundary — no shortening.
3. Name over threshold — shortened correctly.
4. Round-trip: shorten then expand — original restored.
5. Multiple tools shortened — all mappings preserved.
6. Collision detection — two similar long names produce distinct short names.
7. Empty name — handled gracefully.
8. Unicode tool name — shortening respects character boundaries.
9. Forced conversation_id injection — request contains new field.
10. Forced conversation_id with existing id — original preserved.
11. Full convert_request with shortened tools — end-to-end.
12. Response expansion — tool_use blocks in response have original names restored.

**Property-based tests (proptest):**
- For any string S, `expand(shorten(S)) == S` (round-trip invariant).
- For any set of N distinct strings, shortening produces N distinct shortened strings (collision-free invariant).

**Regression:**
- All 56 existing converter tests MUST pass without modification. Run the full `converter.rs` test suite as a regression gate.

## TODOs

- Determine shortening algorithm: hash-based suffix, incremental counter, or prefix+hash.
- Study dev-source tool name shortening implementation for design reference (see SME-03).
- Define max_len threshold from upstream Kiro tool name limits.
