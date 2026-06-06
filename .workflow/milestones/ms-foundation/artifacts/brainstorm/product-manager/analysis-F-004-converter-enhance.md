# F-004 — Converter Layer Enhancement

> Role: product-manager | Related decisions: PM-04, SME-03

## Architecture

The converter layer (anthropic/converter.rs) is the protocol bridge between Anthropic and Kiro formats. Two enhancements from dev-source provide measurable value:

1. **Tool Name Shortening**: MCP tool names can exceed Kiro's name length limits. Dev-source implements automatic abbreviation with a bidirectional mapping table, enabling transparent truncation on the request path and restoration on the response path. The current project already normalizes JSON Schema issues (required: null, properties: null) but lacks name length handling.

2. **Forced conversation_id**: Dev-source injects a conversation_id into every request to maximize upstream cache hits. This directly supports F-001 (cross-request cache) by ensuring the upstream Kiro service can associate requests from the same conversation.

Both enhancements are P1: they build on P0 foundations (particularly F-008 CredentialIdentity for conversation_id derivation) and improve cost efficiency and compatibility.

## Interface Contract

- **Tool Name Shortening**:
  - Input: Anthropic tool definitions with arbitrary-length names
  - Output: Kiro-compatible tool definitions with shortened names + mapping table
  - Response path: restore original tool names before returning to client
- **Forced conversation_id**:
  - Input: Request context (credential identity, message prefix)
  - Output: conversation_id field injected into Kiro request
  - Dependency: F-001 cache and F-008 CredentialIdentity

## Constraints (RFC 2119)

- MUST implement bidirectional tool name mapping (shorten on request, restore on response)
- MUST preserve tool name uniqueness after shortening (collision detection)
- MUST inject conversation_id using the CredentialIdentity-derived identifier from F-008
- MUST NOT break existing JSON Schema normalization (required/properties fixes)
- SHOULD log shortened tool name mappings when sensitive-logs feature is enabled
- MAY implement tool name shortening as a configurable feature (opt-out via config)

## Test Approach

- Unit tests for tool name shortening: long names truncated correctly, short names unchanged
- Unit tests for collision detection: two tools with same shortened prefix get distinct short names
- Unit tests for bidirectional mapping: request shortening + response restoration roundtrips correctly
- Integration test: end-to-end request with long MCP tool names produces valid Kiro request and correct Anthropic response

## TODOs

- Study dev-source converter/tools.rs for shortening algorithm (prefix + hash suffix pattern)
- Measure prevalence of long tool names in real-world MCP tool registrations
- Determine maximum tool name length accepted by Kiro upstream
- Coordinate with SA role on converter.rs modularization (F-007) timeline alignment
