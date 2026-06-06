---
document: architecture
session_id: BLP-kiro-fusion-2026-06-05
adr_id: ADR-004
title: Metrics Ring Buffer Architecture
status: proposed
created: 2026-06-05
deciders:
  - fusion-blueprint-generator
traces:
  - brainstorm/decisions.md#metrics-architecture
---

# ADR-004: Metrics Ring Buffer Architecture

## Context

The current kiro.rs project has zero structured observability. Operational visibility
is limited to log output (and only when `sensitive-logs` feature is enabled at compile time).

The dev-source implements a ring buffer with Admin API aggregation, providing:
- Fixed-size in-memory storage for recent metric records.
- On-demand windowed aggregation (last 1h, 24h, 7d) via Admin API endpoints.
- No external dependencies (no Prometheus, no StatsD, no database).

This aligns with the project's single-binary deployment philosophy. The fusion
MUST adopt this pattern while integrating with the new `ErrorClass` system (ADR-003)
and `CredentialIdentity` (ADR-001).

## Decision

We MUST implement a fixed-size ring buffer for metrics storage with the following design:

```rust
pub struct MetricsCollector {
    buffer: parking_lot::Mutex<RingBuffer<MetricRecord>>,
    config: MetricsConfig,
}

pub struct MetricRecord {
    pub timestamp: Instant,
    pub credential_id: String,
    pub model: String,
    pub status_code: u16,
    pub latency: Duration,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub error_class: Option<ErrorClass>,  // From ADR-003
    pub cache_hit: bool,                  // From ADR-002
}

impl MetricsCollector {
    /// Record a completed request. O(1) — overwrites oldest entry on full buffer.
    pub fn record(&self, record: MetricRecord);

    /// Aggregate metrics over a time window. O(n) scan of buffer.
    pub fn aggregate(&self, window: Duration) -> MetricsAggregate;
}
```

**Key properties:**

- **Fixed size**: 10,000 entries (configurable via `metrics.buffer_size`). At 100 req/s,
  this covers ~100 seconds of history; at typical usage (~1 req/s), ~2.7 hours.
- **O(1) writes**: Ring buffer overwrites oldest entry; no allocation after initialization.
- **O(n) reads**: Aggregation scans the buffer on demand. With 10K entries and simple
  arithmetic aggregation, this completes in <1ms.
- **Thread safety**: `parking_lot::Mutex` wrapping the entire buffer. Write contention is
  minimal because `record()` is a single array write.
- **No persistence**: Metrics reset on service restart. This is acceptable because the
  ring buffer captures recent operational trends, not historical analytics.

**Admin API endpoints:**

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/admin/metrics/summary` | GET | Aggregated metrics for configurable window |
| `/admin/metrics/credentials` | GET | Per-credential breakdown |
| `/admin/metrics/errors` | GET | Error class distribution |
| `/admin/metrics/cache` | GET | Cache hit/miss rates |

**Integration points:**

- `handlers.rs`: Records `MetricRecord` after each request completes.
- `ErrorMapper` (ADR-003): Provides `error_class` for the record.
- `CrossRequestCache` (ADR-002): Provides `cache_hit` flag.
- `Admin Service` (`admin/service.rs`): Calls `aggregate()` for API responses.

## Alternatives Considered

### (a) Prometheus Exporter

Expose metrics in Prometheus format; let external Prometheus scrape.

**Rejected**: Adds an external dependency (Prometheus server) to the deployment. Violates
the single-binary, zero-dependency operational model. Prometheus is overkill for a proxy
service with ~10 metric types.

### (b) SQLite Persistence

Store metrics in an embedded SQLite database for historical queries.

**Rejected**: Adds disk I/O on every request, SQLite dependency, and schema migration
complexity. The ring buffer's bounded memory and ephemeral nature are features, not
limitations, for this use case.

### (c) Structured Log-Only

Emit structured JSON logs and rely on external log aggregation (ELK, Loki).

**Rejected**: Requires external infrastructure for any operational visibility. The Admin
API MUST provide self-contained observability for single-node deployments.

## Consequences

**Positive:**
- Zero external dependencies; bounded, predictable memory usage.
- Sub-millisecond aggregation for Admin API responses.
- `MetricRecord` struct integrates naturally with `ErrorClass` and cache hit data.
- On-demand computation avoids continuous aggregation overhead.

**Negative:**
- No historical data beyond the buffer window.
- Single-node only; no cross-instance aggregation for multi-instance deployments.
- Aggregation is O(n) per request; MAY need caching if Admin API is polled frequently.

**Risks:**
- High request rates could cause the buffer to wrap too quickly, losing older data.
  Mitigation: `buffer_size` is configurable; operators MAY increase it for high-traffic
  deployments.
- Mutex contention under extreme write load. Mitigation: `parking_lot::Mutex` is
  optimized for short critical sections; `record()` is a single array write (~10ns).

## Implementation Notes

- `MetricsCollector` MUST be wrapped in `Arc` and stored in `AppState`.
- `RingBuffer` SHOULD be a simple `Vec<MetricRecord>` with a write index and wrap-around.
- Aggregation MUST skip entries outside the requested time window.
- `MetricRecord` size is ~120 bytes; 10K entries = ~1.2MB memory footprint.
- Unit tests MUST verify wrap-around behavior and windowed aggregation correctness.
