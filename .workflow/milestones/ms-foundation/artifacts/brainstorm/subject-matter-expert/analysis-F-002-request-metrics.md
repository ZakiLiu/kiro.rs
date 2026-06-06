# F-002 — Request Metrics and Observability

> Role: subject-matter-expert | Related decisions: SME-04, PM-01, SA-03

## Architecture

The current project has zero metrics infrastructure — no latency tracking, no TTFB measurement, no error classification histograms. The dev-source implements a ring-buffer metrics collector (`metrics.rs`) with Admin API aggregation (`admin/metrics.rs`). The SME perspective focuses on what domain-specific metrics matter and how they interact with existing anti-detection and compression subsystems.

Proposed module: `anthropic/metrics.rs` — a lock-free ring buffer (fixed capacity, e.g., 4096 entries) storing per-request telemetry. Each entry captures:
- Request timestamp, model, credential_id
- Total latency (end-to-end), TTFB (time to first byte in streaming)
- Cache status (from F-001): hit/miss/partial
- Compression stats (from `CompressionStats`): bytes saved per stage
- Error classification: upstream error code, mapped Anthropic error type
- Stream completion status: completed / aborted / timeout

Admin API endpoint: `GET /admin/metrics` with query params for time window and model filter.

## Interface Contract

```rust
pub struct RequestMetric {
    pub timestamp: DateTime<Utc>,
    pub model: String,
    pub credential_id: u64,
    pub latency_ms: u64,
    pub ttfb_ms: Option<u64>,
    pub cache_hit: bool,
    pub cache_read_tokens: i32,
    pub compression_saved_bytes: usize,
    pub error_type: Option<String>,
    pub stream_status: StreamStatus,
}

pub enum StreamStatus { Completed, Aborted, Timeout, NonStreaming }

pub struct MetricsCollector {
    ring: RingBuffer<RequestMetric>,
}

impl MetricsCollector {
    pub fn record(&self, metric: RequestMetric);
    pub fn query(&self, window: Duration) -> MetricsSummary;
    pub fn query_by_model(&self, model: &str, window: Duration) -> MetricsSummary;
}
```

Consumers: `anthropic/handlers.rs` (recording), `admin/handlers.rs` (querying), `anthropic/stream.rs` (TTFB capture).

## Constraints (RFC 2119)

- Metrics collection MUST NOT add more than 1ms overhead to request latency — use lock-free or try-lock patterns.
- Metrics MUST cover all five domain-critical signals: latency, TTFB, cache hit rate, compression ratio, and error distribution.
- Metrics SHOULD integrate with the compression pipeline to report per-stage savings.
- Metrics MUST NOT expose credential-identifying information through the Admin API — aggregate by credential index, not by token content.
- The ring buffer SHOULD use a power-of-two capacity for efficient modular indexing.
- Metrics MAY support Prometheus-compatible text format export in a future iteration.

## Test Approach

- Unit tests: Ring buffer wrap-around correctness, concurrent write safety under `tokio::test` with 100 concurrent writers.
- Integration tests: Record 50 metrics, query with different time windows, verify aggregation accuracy.
- Performance benchmark: Measure `record()` latency under load (target < 500ns per call).

## TODOs

- Decide between `AtomicUsize` cursor vs `parking_lot::Mutex` for ring buffer — benchmark both.
- Study dev-source `metrics.rs` for the Admin API response schema to maintain compatibility.
- Determine whether TTFB should be measured from request dispatch or from first upstream byte arrival.
