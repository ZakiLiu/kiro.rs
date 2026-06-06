# F-004 -- Converter Layer Enhancement

> Role: system-architect | Related decisions: SA-08, SME-03, PM-04

## Architecture

Two enhancements to the existing `src/anthropic/converter.rs`:

### Tool Name Shortening

Some Claude Code and MCP tool names exceed upstream length limits, causing 400 errors. The converter SHOULD detect tool names exceeding a threshold (e.g., 64 characters) and replace them with shortened hashes. A request-scoped ToolNameMap tracks original-to-shortened mappings so that response tool_use events can be reverse-mapped before returning to the client.

**Data flow:**
1. converter::convert_request() scans tools array for long names
2. Long names are replaced with a deterministic short form: first 48 chars + "_" + 8-char hash suffix
3. ToolNameMap is stored in request extensions (Axum Extension layer)
4. stream.rs reads ToolNameMap from extensions and reverse-maps tool names in response events

### Forced conversation_id

When CrossRequestCache (F-001) provides a cached conversation_id, the converter MUST inject it into the Kiro request payload. This is a straightforward field injection in the conversation request body, placed after standard conversion but before signing.

**Module impact:** Both enhancements modify converter.rs internals. Given SA-01, this is a good candidate for splitting converter.rs into sub-modules during the F-007 refactor. The tool shortening logic could become `converter/tools.rs` and conversation_id injection could become `converter/session.rs` (mirroring dev-source structure).

## Interface Contract

```rust
// Tool name shortening
pub struct ToolNameMap {
    mapping: HashMap<String, String>,  // shortened -> original
}

impl ToolNameMap {
    pub fn shorten_if_needed(name: &str) -> (String, bool);
    pub fn reverse(&self, shortened: &str) -> Option<String>;
}

// Conversation ID injection
pub fn inject_conversation_id(request: &mut KiroRequest, conversation_id: &str);
```

The ToolNameMap is attached to the Axum request via `request.extensions_mut().insert(tool_name_map)` and retrieved in stream.rs via `request.extensions().get::<ToolNameMap>()`.

## Constraints (RFC 2119)

- MUST produce deterministic shortened names (same input always yields same output)
- MUST maintain a reversible mapping for response reconstruction
- MUST NOT shorten names that are already within the length limit
- SHOULD use a hash-based suffix to minimize collision risk
- MUST inject conversation_id only when CrossRequestCache provides a cached value
- MUST NOT inject conversation_id for new conversations (cache miss)
- SHOULD log tool name shortening events at DEBUG level for troubleshooting

## Test Approach

- **Unit tests:** Name shortening determinism. Reverse mapping correctness. Names at boundary length. Hash collision probability (statistical test over 10000 random names).
- **Integration tests:** Full request/response cycle with long tool names -- verify client receives original names. Conversation_id injection produces valid upstream request.
- **Regression:** Verify that requests with short tool names are completely unaffected.
- **Edge cases:** Tool name with special characters. Empty tool name. Tool name exactly at length limit.

## TODOs

- Determine the upstream tool name length limit (currently assumed 64, needs verification)
- Evaluate whether the shortening algorithm should be pluggable or fixed
- Decide on request extension vs explicit parameter passing for ToolNameMap lifecycle
- Coordinate with F-007 module-refactor on splitting converter.rs sub-modules
