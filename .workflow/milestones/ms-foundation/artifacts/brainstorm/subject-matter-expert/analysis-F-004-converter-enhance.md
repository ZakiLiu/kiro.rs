# F-004 — Converter Layer Enhancement

> Role: subject-matter-expert | Related decisions: SME-03

## Architecture

The converter layer (`anthropic/converter.rs`, ~1000+ lines) handles Anthropic-to-Kiro protocol translation. Two specific enhancements are required per SME-03:

### Tool Name Shortening

MCP tool names frequently exceed Kiro's upstream limits (e.g., `mcp__filesystem__read_file` is 30+ chars). The dev-source implements automatic shortening: names over a threshold are hashed to a short alias, and a bidirectional mapping table is maintained for the request-response lifecycle.

Implementation strategy:
- During `convert_request()`, scan tool definitions for names exceeding 32 characters.
- Generate a deterministic short name: `t_{sha256(original_name)[0..8]}` (12 chars total).
- Store the mapping `short_name <-> original_name` in a per-request `ToolNameMap`.
- During response stream processing (`stream.rs`), reverse-map tool names back to originals before returning to the client.
- The mapping MUST be deterministic (same original name always produces the same short name) to support prefix cache reuse.

### Forced Conversation ID

When `conversation_id` is known from the cross-request cache (F-001), the converter MUST inject it into the Kiro request's `ConversationState`. This enables upstream prefix cache reuse without the client needing to manage conversation state.

Implementation:
- Accept an optional `conversation_id: Option<String>` parameter in `convert_request()`.
- When present, set `ConversationState.conversation_id = Some(conversation_id)` regardless of whether the client provided one.
- When absent, let the upstream assign a new conversation_id (current behavior).

## Interface Contract

```rust
pub struct ToolNameMap {
    short_to_original: HashMap<String, String>,
    original_to_short: HashMap<String, String>,
}

impl ToolNameMap {
    pub fn shorten(&mut self, name: &str) -> String;
    pub fn restore(&self, short_name: &str) -> Option<&str>;
}

// Modified convert_request signature:
pub fn convert_request(
    request: &MessagesRequest,
    conversation_id: Option<String>,
    tool_name_map: &mut ToolNameMap,
    // ... existing params
) -> ConversationState;
```

Consumers: `anthropic/handlers.rs` (request path), `anthropic/stream.rs` (response path for name restoration).

## Constraints (RFC 2119)

- Tool name shortening MUST be deterministic — the same input name MUST always produce the same short name, enabling prefix cache stability.
- Tool name shortening MUST only activate for names exceeding 32 characters — short names pass through unchanged.
- The short name format MUST avoid collision with legitimate tool names — the `t_` prefix with hex hash provides this guarantee.
- Response stream processing MUST restore original tool names before returning to the client — the client MUST NOT see shortened names.
- Forced conversation_id injection MUST NOT override client-provided conversation state if the upstream protocol supports client-side conversation management in the future.
- The `ToolNameMap` SHOULD be logged (at debug level) for troubleshooting tool name resolution failures.

## Test Approach

- Unit tests: Shortening determinism (same input -> same output across calls), collision resistance (10,000 unique names, zero collisions), round-trip fidelity (shorten -> restore == original).
- Integration tests: Full request-response cycle with shortened tool names — verify client sees original names in the response.
- Edge cases: Empty tool name, tool name exactly at threshold (32 chars), Unicode tool names, tool name containing `t_` prefix naturally.

## TODOs

- Measure the shortening threshold impact: at 32 chars, how many MCP tools in a typical Claude Code session exceed this?
- Study whether Kiro upstream has an explicit tool name length limit or if this is inferred from 400 error patterns.
- Verify that `conversation_id` injection does not break the existing `agentContinuationId` UUID v5 generation logic.
