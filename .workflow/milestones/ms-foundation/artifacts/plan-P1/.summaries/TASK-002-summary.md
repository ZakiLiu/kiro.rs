# TASK-002: Implement MetricsCollector with ring buffer and AppState integration

## Changes
- `src/metrics.rs`: Created new module with `MetricEventType` enum (RequestReceived, CredentialSelected, RequestCompleted), `MetricEvent` struct with builder methods, and `MetricsCollector` with `parking_lot::Mutex<VecDeque<MetricEvent>>` ring buffer
- `src/model/config.rs`: Added `metrics_enabled: bool` (default true) and `metrics_ring_buffer_size: usize` (default 10000) with serde defaults to Config struct
- `src/anthropic/middleware.rs`: Added `pub metrics: Option<Arc<MetricsCollector>>` field to AppState, `with_metrics()` builder method, initialized as None in `new()`
- `src/anthropic/router.rs`: Added `metrics: Option<Arc<MetricsCollector>>` parameter to `create_router_with_provider()`, passes to AppState
- `src/anthropic/handlers.rs`: Emits `RequestReceived` after "Received request" log in `post_messages`, emits `RequestCompleted` with latency/tokens at end of both `handle_stream_request` and `handle_non_stream_request`
- `src/main.rs`: Registered `pub mod metrics;`, constructs `MetricsCollector` from config and passes to router

## Verification
- [x] File src/metrics.rs exists with MetricsCollector containing Mutex<VecDeque<MetricEvent>>: Created and verified
- [x] MetricEvent has all required fields (timestamp, event_type, model, credential_id, latency_ms, status, input_tokens, output_tokens): Implemented with builder pattern
- [x] MetricsCollector::record() pushes to ring buffer and evicts oldest when full: Verified by test_ring_buffer_evicts_oldest
- [x] MetricsCollector::snapshot() returns Vec<MetricEvent> clone: Verified by test_snapshot_returns_clone
- [x] Config has metrics_enabled and metrics_ring_buffer_size with serde defaults: Added with default_true and default_metrics_ring_buffer_size
- [x] AppState has metrics: Option<Arc<MetricsCollector>>: Added with with_metrics() builder
- [x] handlers.rs emits RequestReceived at request entry: Added after "Received request" log
- [x] handlers.rs emits RequestCompleted after stream and non-stream response: Added at end of both handlers
- [x] main.rs constructs MetricsCollector and passes to AppState: Conditional on config.metrics_enabled
- [x] cargo test passes with ring buffer tests: 6 tests pass (record, snapshot, eviction, empty, max_size_one, builder)

## Tests
- [x] `cargo test -- metrics`: 6 passed, 0 failed
- [x] `cargo clippy`: 0 new warnings (5 pre-existing warnings in converter.rs and handlers.rs unrelated to this task)
- [x] `cargo build`: Compiles successfully
- [x] `cargo test` (full): 407 passed, 0 failed

## Deviations
- `cargo clippy -- -D warnings` fails due to 5 pre-existing warnings in converter.rs and handlers.rs (collapsible_if, doc_lazy_continuation, collapsible_match, unnecessary_cast). None are introduced by this task.
- Module registered in main.rs (as `pub mod metrics;`) rather than a separate lib.rs, following the existing codebase pattern where main.rs serves as the crate root.

## Notes
- The `CredentialSelected` event type is defined but not emitted in this task; it's intended for future use when credential selection metrics are needed.
- Stream handler records `RequestCompleted` at time-to-first-byte (when API response arrives), not at stream end. Output tokens for stream responses are recorded as 0 since the full count isn't known until stream completion.
- The `request_start` timestamp is captured once in `post_messages` and threaded through context structs to avoid duplicating timing logic.
