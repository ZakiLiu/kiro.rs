---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-004
priority: must
wave: P1
---

# REQ-004: Converter Enhancement — Tool Shortening and Conversation ID Injection

## User Story

As a **Power User**, I want MCP tools with long names to be automatically shortened to fit Kiro's character limits, and I want cached conversation_id to be transparently injected into requests, so that I can use complex tool ecosystems without manual workarounds and benefit from prefix cache reuse.

## Description

The current `converter.rs` handles Anthropic-to-Kiro protocol conversion including model mapping, JSON Schema normalization, and tool placeholder generation. Two enhancements are needed:

**Tool Name Shortening**: When MCP tool names exceed Kiro's character limit, the converter generates a deterministic short name by computing SHA-256 of the original name and truncating to 8 hex characters (e.g., `mcp__github__create_pull_request` → `a7f2b3c1`). The mapping is stored in a request-scoped `ToolNameMap` that travels with the request context. During response processing in `stream.rs`, the map is used to restore original tool names before returning to the client. The mapping MUST be deterministic — the same original name MUST always produce the same short name.

**Conversation ID Injection**: When the cross-request cache (REQ-001) provides a cached `conversation_id` for the current request, the converter MUST inject it into the Kiro request's `conversationId` field. This enables upstream prefix cache reuse. The injection point is in `convert_request()`, after all other request transformations are complete. If no cached conversation_id is available, the field is omitted (default upstream behavior: new conversation).

## Acceptance Criteria

1. **Deterministic Shortening**: Tool names exceeding the upstream character limit MUST be shortened using SHA-256 truncated to 8 hex characters. The same original name MUST always produce the same shortened name. The shortening MUST be applied during `convert_request()` and reversed during response processing.

2. **Reversible Mapping**: A `ToolNameMap` MUST be created per-request storing all `(original, shortened)` pairs. During streaming response processing in `stream.rs`, every `tool_use` event referencing a shortened name MUST be restored to the original name before client delivery. If a shortened name has no mapping entry, it MUST be passed through unchanged (defensive behavior).

3. **Conversation ID Injection**: When a cached `conversation_id` is provided (from REQ-001 lookup), `convert_request()` MUST set the `conversationId` field in the outgoing Kiro request. When no cached value exists, the field MUST be omitted entirely.

4. **Collision Monitoring**: The 8-hex-char truncation has a theoretical collision space of 2^32. Tool name collision (two different original names producing the same short name) SHOULD be detected at conversion time and logged as a warning. If collision is detected, the system SHOULD fall back to 12 hex characters for the colliding name.

## Data Model

```rust
pub struct ToolNameMap {
    forward: HashMap<String, String>,  // original → shortened
    reverse: HashMap<String, String>,  // shortened → original
}

impl ToolNameMap {
    pub fn shorten(&mut self, name: &str) -> String;
    pub fn restore(&self, short: &str) -> Option<&str>;
    pub fn is_empty(&self) -> bool;
}
```

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-001 | **Soft** — Receives cached conversation_id to inject |
| REQ-008 | **Indirect** — CredentialIdentity provides the cache key that enables conversation_id lookup |
| REQ-007 | **Soft** — converter.rs split may relocate this logic into a sub-module |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-08 | System Architect | Tool name shortening + conversation_id injection design |
| PM-04 | Product Manager | P1 wave — reliability enhancement |
| SME-03 | Subject Matter Expert | Domain-justified: Kiro has character limits on tool names |
| TS-06 | Test Strategist | Round-trip fidelity tests required |

## Edge Cases

| Scenario | Behavior |
|----------|----------|
| Tool name already short enough | Pass through unchanged, no mapping entry created |
| Multiple tools with same SHA-256 prefix | Detect collision, extend to 12 hex chars for the later name |
| Response references unknown short name | Pass through unchanged, log warning |
| Conversation ID injection + new conversation | Omit field — upstream creates new conversation |
| Conversation ID injection + expired cache entry | Cache returns None, converter omits field |

## Integration Points

- **Request path**: `handlers.rs` → cache lookup → `converter.rs` (inject conversation_id, shorten tools) → `provider.rs`
- **Response path**: `stream.rs` (restore tool names using ToolNameMap) → client
- **ToolNameMap** is stored in request-scoped context (Axum Extension or function parameter), not in global state
