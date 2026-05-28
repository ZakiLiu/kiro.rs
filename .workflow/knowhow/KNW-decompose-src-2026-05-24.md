---
related:
  - "spec:project:architecture-constraints"
  - "spec:project:coding-conventions"
  - knowhow-periodic-recovery-2026-05-25
---

# Pattern Decomposition: src/ (Full Codebase)
**Generated:** 2026-05-24 | **Files analyzed:** 63 | **Patterns found:** 40 raw → 24 unique (after dedup)

## Summary

| Dimension | Raw findings | After dedup | Documented | New |
|-----------|-------------|-------------|------------|-----|
| Structural | 10 | 8 | 3 | 5 |
| Behavioral | 10 | 7 | 1 | 6 |
| Data | 10 | 8 | 0 | 8 |
| Error | 10 | 7 | 2 | 5 |
| **Total** | **40** | **30** | **6** | **24** |

Cross-dimension merges: 5 patterns appeared in 2+ dimensions.

---

## Documented Patterns (already in coding-conventions.md)

| Pattern | Status | Convention Reference |
|---------|--------|---------------------|
| Arc-Wrapped Shared State | documented | "Concurrency: Arc&lt;RwLock&lt;T&gt;&gt; (parking_lot)" |
| Builder Pattern (AppState) | documented | "Builder pattern: struct with new() constructor" |
| Module Barrel Exports | documented | "Module organization: mod.rs as barrel" |
| Custom Error Enums | documented | "Error handling: custom error types for library boundaries" |
| Axum Middleware Chain | documented | "Async: Tokio runtime, async fn handlers" |
| DI via Axum State Extractor | documented | Implied by Axum patterns |

---

## New Patterns — Structural

### S1. Trait Object Dispatch for Endpoint Polymorphism
- **Confidence:** high
- **Anchors:** `src/kiro/endpoint/mod.rs:20-42`, `src/kiro/provider.rs:55-57`
- **Description:** `KiroEndpoint` trait with `Arc<dyn KiroEndpoint>` stored in HashMap registry. IdeEndpoint and CliEndpoint are concrete strategies. Per-credential endpoint override enables mixed pools.
- **Rationale:** Runtime endpoint selection without monomorphization bloat. Open/closed principle.
- **Tradeoffs:** +extensible, +runtime flexible | -vtable overhead, -String key allocation

### S2. Enum Dispatch for Unified Event Parsing
- **Confidence:** high
- **Anchors:** `src/kiro/model/events/base.rs:63-95`, `src/kiro/model/events/mod.rs:11-16`
- **Description:** `Event` enum with typed variants (AssistantResponse, ToolUse, Metering, etc.). EventPayload trait decouples parsing. Unknown variant as escape hatch.
- **Rationale:** Zero-cost dispatch, exhaustive match enforcement by compiler.
- **Tradeoffs:** +zero-cost, +exhaustive | -closed for extension, -Unknown swallows unrecognized events

### S3. Snapshot Pattern for Lock-Free Read Access
- **Confidence:** high
- **Anchors:** `src/kiro/token_manager.rs:585-627`, `src/admin/service.rs:88`
- **Description:** `snapshot()` clones credential entries into a separate DTO struct, releasing the lock immediately. Admin reads never contend with hot-path writes.
- **Rationale:** Prevents lock contention between admin reads and credential selection.
- **Tradeoffs:** +zero contention, +clean public/private boundary | -stale data, -O(n) clone

### S4. Atomic State Machine for Background Task Lifecycle
- **Confidence:** medium
- **Anchors:** `src/kiro/background_refresh.rs:68-75`, `src/kiro/background_refresh.rs:100-115`
- **Description:** AtomicBool + Notify for lifecycle control. Generic closure injection for refresh logic. swap(true, SeqCst) as compare-and-set to prevent double-start.
- **Rationale:** Lighter than Mutex&lt;Option&lt;JoinHandle&gt;&gt;. Inversion of control via closures.
- **Tradeoffs:** +lightweight, +decoupled | -small race window, -verbose closure types

### S5. Serde Untagged Enum for Backward-Compatible Config
- **Confidence:** high
- **Anchors:** `src/kiro/model/credentials.rs:132-140`
- **Description:** `CredentialsConfig` with Single/Multiple variants. serde tries each in order. `into_sorted_credentials()` normalizes to Vec.
- **Rationale:** Zero-migration backward compatibility for legacy single-credential files.
- **Tradeoffs:** +no migration needed | -opaque error messages, -large_enum_variant warning

---

## New Patterns — Behavioral

### B1. SSE State Machine (SseStateManager)
- **Confidence:** high
- **Anchors:** `src/anthropic/stream.rs:219-429`
- **Description:** Enforces Anthropic SSE event ordering (message_start → content_block_start → delta* → stop → message_delta → message_stop). Auto-closes unclosed blocks. Stop-reason priority via static ordered slice.
- **Rationale:** Upstream Kiro events don't guarantee Anthropic ordering. Protocol adapter normalizes the stream.
- **Tradeoffs:** +robust against malformed upstream | -stateful per-stream, -hardcoded priority list

### B2. Four-State Decoder State Machine
- **Confidence:** high
- **Anchors:** `src/kiro/parser/decoder.rs:54-242`
- **Description:** Ready → Parsing → Recovering → Stopped. Error-type-specific skip strategies (prelude: skip 1 byte; data: skip whole frame). TooManyErrors terminal state.
- **Rationale:** Self-healing binary stream parser. Partial/corrupted frames don't abort the entire stream.
- **Tradeoffs:** +resilient, +bounded errors | -Stopped is terminal (try_resume is dead code)

### B3. Strategy — Auth Method Dispatch (Social vs IdC)
- **Confidence:** high
- **Anchors:** `src/kiro/token_manager.rs:438`, `src/kiro/token_manager.rs:143`
- **Description:** Dispatches on `auth_method` to select OAuth (Social) or AWS OIDC (IdC) refresh flow. API-key credentials bypass expiry entirely.
- **Rationale:** Multiple auth backends with different lifetimes/endpoints. Provider layer stays auth-agnostic.
- **Tradeoffs:** +extensible | -string-based dispatch (not type-safe)

### B4. Multi-Layer Adaptive Compression Pipeline ⭐
- **Confidence:** high
- **Anchors:** `src/anthropic/compressor.rs:37-80`, `src/anthropic/handlers.rs:238-446`
- **Description:** 5 ordered passes (whitespace → thinking → tool_result → tool_use → history) in a feedback loop (up to 32 iterations). ¾ geometric reduction per iteration. Repair passes fix orphaned pairs.
- **Rationale:** Upstream ~5MiB limit. Finds minimum compression needed to fit.
- **Tradeoffs:** +preserves max context, +configurable | -up to 32 re-serializations, -caller has no visibility into dropped context

### B5. Observer — Dual Background Balance Refresh
- **Confidence:** high
- **Anchors:** `src/kiro/provider.rs:196-244`, `src/kiro/token_manager.rs:706`
- **Description:** Periodic ticker (600s) + fire-and-forget post-success refresh. Dynamic TTL (10min/30min/24h) based on usage frequency and balance level.
- **Rationale:** Balance data needed for load-balancing but expensive to fetch synchronously.
- **Tradeoffs:** +non-blocking, +adaptive TTL | -stale up to TTL, -two refresh paths can race

### B6. Command Pattern — Request Context Decomposition
- **Confidence:** medium
- **Anchors:** `src/anthropic/handlers.rs:52-63`
- **Description:** StreamRequestContext / NonStreamRequestContext encapsulate all parameters for each execution path. Separates "what to do" from "how to do it".
- **Rationale:** Streaming and non-streaming paths share setup but diverge in consumption.
- **Tradeoffs:** +independently testable | -local to handlers.rs, more "parameter object" than true Command

---

## New Patterns — Data

### D1. Bidirectional Protocol Translation Pipeline ⭐
- **Confidence:** high
- **Anchors:** `src/anthropic/converter.rs:200-408`
- **Description:** 9-step numbered pipeline: model mapping → message validation → prefill stripping → image budget → current message extraction → tool conversion → history building → tool pairing → orphan cleanup. Reverse path in stream.rs.
- **Rationale:** Strict protocol isolation. Each step independently testable.
- **Tradeoffs:** +clear separation, +auditable | -very long function (~600 lines), -ordering-sensitive

### D2. Dynamic TTL Balance Cache
- **Confidence:** high
- **Anchors:** `src/kiro/token_manager.rs:649-671`, `src/kiro/token_manager.rs:1534-1550`
- **Description:** CachedBalance with Instant-based TTL computed at read time. Low-balance: 24h, high-frequency (≥20 uses/10min): 10min, default: 30min.
- **Rationale:** Avoids hammering balance API for low-balance credentials.
- **Tradeoffs:** +adaptive refresh | -TTL logic duplicated in 3 places, -can't survive restart

### D3. JSON Schema Normalization/Sanitization
- **Confidence:** high
- **Anchors:** `src/anthropic/converter.rs:55-130`
- **Description:** Fixes MCP tool definitions: null type → "object", null properties → {}, null required → []. Does NOT add $schema when absent (wire-alignment with kiro-cli 2.3.0).
- **Rationale:** MCP tools frequently violate JSON Schema constraints. Prevents upstream 400s.
- **Tradeoffs:** +transparent fix | -silently mutates, -subtle "don't add $schema" rule easy to regress

### D4. AWS Event Stream Binary Frame Parser
- **Confidence:** high
- **Anchors:** `src/kiro/parser/frame.rs:1-120`
- **Description:** Stateless pure function: 4B total length → 4B header length → 4B prelude CRC32 → headers → payload → 4B message CRC32. Both CRCs verified.
- **Rationale:** CRC verification catches network corruption before silent data corruption.
- **Tradeoffs:** +stateless, +testable | -two-pass CRC adds CPU, -16MB hard limit

### D5. Conversation ID Derivation with Stable Fingerprinting
- **Confidence:** high
- **Anchors:** `src/anthropic/converter.rs:290-380`
- **Description:** Priority: (a) UUID from metadata.user_id, (b) SHA-256 of first message → UUID v5, (c) random UUID v4. First-message anchor stays stable across turns.
- **Rationale:** Stable IDs reduce telemetry noise without stateful session tracking.
- **Tradeoffs:** +stable across turns | -first-message collision possible, -three-tier complexity

### D6. Serde Custom Deserializer for Polymorphic System Field
- **Confidence:** high
- **Anchors:** `src/anthropic/types.rs` (SystemVisitor)
- **Description:** `system` field accepts string or array. Custom Visitor normalizes both to Vec&lt;SystemMessage&gt;.
- **Rationale:** Anthropic API allows both formats. Transparent normalization.
- **Tradeoffs:** +callers never branch | -verbose visitor pattern

### D7. GIF Frame Sampling with Adaptive Rate Control
- **Confidence:** medium
- **Anchors:** `src/image.rs:60-100`
- **Description:** Two-pass: measure duration → sample at adaptive interval (max 20 frames, max 5fps). Re-encode as JPEG.
- **Rationale:** Raw GIFs too large for upstream. Bounded output regardless of input length.
- **Tradeoffs:** +bounded size | -two-pass doubles CPU/memory, -loses animation metadata

### D8. Streaming SSE with Local Token Estimation
- **Confidence:** high
- **Anchors:** `src/anthropic/stream.rs:1364-1382`
- **Description:** estimate_tokens() heuristic: CJK ~1.5 chars/token, Latin ~4 chars/token. Merges local estimates with upstream meteringEvent.
- **Rationale:** Upstream doesn't provide Anthropic-compatible token counts.
- **Tradeoffs:** +always provides usage data | -approximate (±20%), -mixed-script weakness

---

## New Patterns — Error

### E1. Retry Loop with Exponential Backoff + Jitter
- **Confidence:** high
- **Anchors:** `src/kiro/provider.rs:596-993`
- **Description:** BASE_MS * 2^attempt, cap at 2000ms, +25% random jitter. Max 3 total retries. Network errors don't push to failed_ids.
- **Rationale:** Prevents thundering-herd. Separates network errors from credential errors.
- **Tradeoffs:** +bounded tail latency | -3 retries may be too low for large pools

### E2. Circuit Breaker via Global Credential Disable ⭐
- **Confidence:** high
- **Anchors:** `src/kiro/token_manager.rs:2188-2260`
- **Description:** AtomicU32 counter → threshold → disable_all_credentials() → global_recovery_time. check_and_recover() re-enables only ModelUnavailable (not QuotaExceeded). Counter resets on any success.
- **Rationale:** Prevents hammering globally unavailable model. Selective recovery.
- **Tradeoffs:** +automatic recovery | -shared global timer, -single success resets counter

### E3. Credential Failover with Affinity-Aware Exclusion ⭐
- **Confidence:** high
- **Anchors:** `src/kiro/provider.rs:607-633`, `src/kiro/token_manager.rs:1297-1434`
- **Description:** failed_ids exclusion list per-request. Auto-heal when all disabled (resets failure counts). 429 + Retry-After when all cooling down.
- **Rationale:** Prevents single bad credential from blocking all requests.
- **Tradeoffs:** +zero-downtime recovery | -auto-heal retries genuinely broken credentials

### E4. Differentiated Cooldown with Exponential Backoff per Reason
- **Confidence:** high
- **Anchors:** `src/kiro/cooldown.rs:28-279`
- **Description:** Per-credential HashMap&lt;CooldownReason, Instant&gt;. Auto-recoverable: base * 1.5^(count-1), cap 300s. Non-recoverable: fixed 86400s. Retry-After parsed and clamped [60s, 300s].
- **Rationale:** Different failure modes warrant different recovery windows.
- **Tradeoffs:** +fine-grained, +self-healing | -3 "future" variants not yet wired in production

### E5. Error Classification Guard Clauses at HTTP Boundary
- **Confidence:** high
- **Anchors:** `src/anthropic/handlers.rs:448-562`
- **Description:** Cascade of is_*_error() predicates → specific HTTP status + Anthropic JSON body. Network errors suppress request body from logs.
- **Rationale:** Centralizes upstream-error-to-HTTP-status translation.
- **Tradeoffs:** +easy to extend | -string matching is fragile, -nested guards easy to miss

---

## Cross-Dimension Patterns (appeared in 2+ dimensions)

| Pattern | Dimensions | Primary Anchor |
|---------|-----------|----------------|
| Multi-Layer Adaptive Compression | Structural + Behavioral + Data + Error | `src/anthropic/compressor.rs` |
| Credential Failover Chain | Behavioral + Error | `src/kiro/provider.rs:596` |
| Differentiated Cooldown | Behavioral + Error | `src/kiro/cooldown.rs` |
| Serde Untagged Enum | Structural + Data | `src/kiro/model/credentials.rs:132` |
| Background Balance Refresh + Dynamic TTL | Data + Behavioral | `src/kiro/token_manager.rs:649` |

---

## Contradictions with Documented Conventions

| Finding | Convention says | Actual code | Severity |
|---------|----------------|-------------|----------|
| Error handling | "anyhow::Result for application errors" | Provider uses `anyhow::Error` but handlers do string-based classification | low — pragmatic choice given the boundary |
| Builder pattern | "struct with new() constructor" | AppState uses consuming `with_*` methods (not a separate Builder type) | low — variant of the pattern |

---

## Key Insights

1. **Resilience is the dominant architectural theme** — retry, failover, circuit breaker, cooldown, compression, and decoder recovery all serve the same goal: never drop a user request if there's any way to serve it.

2. **Protocol isolation is strict** — Anthropic types never leak into Kiro types. The converter is the only bridge, and it's a numbered pipeline with clear steps.

3. **Adaptive over fixed** — TTL, compression thresholds, cooldown durations, and sampling rates all adjust dynamically based on runtime conditions rather than using fixed values.

4. **String-based error classification is a known debt** — the guard clause pattern works but is fragile. A typed error enum across the full stack would be more robust.

5. **Three "future" cooldown variants are defined but unwired** — AuthenticationFailed, AccountSuspended, QuotaExhausted have durations defined but no production code triggers them yet.
