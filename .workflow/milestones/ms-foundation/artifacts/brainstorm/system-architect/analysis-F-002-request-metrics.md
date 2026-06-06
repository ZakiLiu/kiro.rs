# F-002 -- Request Metrics and Observability

> Role: system-architect | Related decisions: SA-07, PM-01, PM-04, TS-01

## Architecture

A new module `src/common/metrics.rs` provides a centralized MetricsCollector. The design follows the dev-source ring-buffer pattern: a fixed-size circular buffer stores MetricRecord entries, enabling windowed aggregation without unbounded memory growth.

**Module placement:** `src/common/metrics.rs` in the common layer since metrics are consumed by both the Anthropic handler path and the Admin API. The collector is injected into AppState as `Arc<MetricsCollector>`.

**Integration points:**
1. `anthropic/handlers.rs` -- records request start/end timestamps, model, credential_id, cache hit/miss, final status
2. `anthropic/stream.rs` -- records TTFB (time from request dispatch to first SSE event), stream abort detection
3. `kiro/provider.rs` -- records credential selection events for distribution tracking
4. `admin/handlers.rs` -- exposes aggregated metrics via GET /api/admin/metrics endpoint
5. `common/metrics.rs` (new) -- ErrorMapper integration for error classification counts

**Ring buffer design:** VecDeque with fixed capacity. On insert, if len == capacity, pop_front. This provides O(1) insert and O(n) window queries. For the default 10000 entries, a full scan takes microseconds.

**Admin API exposure:** New endpoint `GET /api/admin/metrics?window=300` returns aggregated stats for the specified time window (seconds). Response includes: total_requests, avg_latency_ms, p50/p95/p99 latency, cache_hit_rate, error_breakdown, per_credential_distribution, per_model_breakdown.

## Interface Contract

```rust
pub struct MetricsCollector {
    buffer: parking_lot::Mutex<VecDeque<MetricRecord>>,
    capacity: usize,
}

pub struct MetricRecord {
    pub timestamp: Instant,
    pub latency_ms: u64,
    pub ttfb_ms: Option<u64>,
    pub credential_id: u64,
    pub model: String,
    pub status: u16,
    pub cache_hit: bool,
    pub error_class: Option<ErrorClass>,
    pub stream_aborted: bool,
}

pub struct MetricsSnapshot {
    pub window_seconds: u64,
    pub total_requests: u64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: u64,
    pub p95_latency_ms: u64,
    pub p99_latency_ms: u64,
    pub cache_hit_rate: f64,
    pub error_breakdown: HashMap<ErrorClass, u64>,
    pub credential_distribution: HashMap<u64, u64>,
    pub model_breakdown: HashMap<String, u64>,
}

impl MetricsCollector {
    pub fn new(capacity: usize) -> Self;
    pub fn record(&self, record: MetricRecord);
    pub fn query(&self, window: Duration) -> MetricsSnapshot;
    pub fn total_count(&self) -> u64;
}
```

## Constraints (RFC 2119)

- MUST use a fixed-size ring buffer; MUST NOT grow unboundedly
- MUST record at minimum: latency, TTFB, credential_id, model, status, cache_hit
- SHOULD compute percentile latencies (p50, p95, p99) on query, not on insert
- MUST NOT introduce measurable latency on the request hot path (record is O(1))
- MUST expose metrics via Admin API only (admin_api_key required)
- SHOULD support configurable ring buffer size and default query window
- MAY flush metrics to tracing log at shutdown for post-mortem analysis

## Test Approach

- **Unit tests:** Ring buffer insert/eviction. Window-based query with known timestamps. Percentile calculation accuracy. Empty buffer edge case.
- **Integration tests:** Full request cycle with metrics recording. Admin API endpoint returns correct aggregation. Verify metrics survive credential failover.
- **Load tests:** Verify O(1) insert performance under concurrent writes. Measure query latency with full 10000-entry buffer.
- **Compatibility:** Verify existing Admin API endpoints are unaffected by new metrics endpoint addition.

## TODOs

- Determine serialization format for Admin API response (JSON is obvious, but consider Prometheus text format as optional)
- Evaluate whether per-model breakdown needs a separate counter or can be derived from ring buffer scan
- Decide on clock source: Instant (monotonic, no wall-clock) vs SystemTime (needed for Admin UI display)
- Consider whether stream_aborted detection requires cooperation from the Axum response body or can be detected from hyper disconnect
