# F-002 — Request Metrics and Observability

> Role: product-manager | Related decisions: PM-01, PM-04, TS-01

## Architecture

Observability is the current project's most significant operational gap. Dev-source implements a ring buffer metrics system (metrics.rs) with Admin aggregation (admin/metrics.rs) covering request latency, TTFB, stream abort detection, and per-model breakdowns. The current project has zero runtime metrics — operators fly blind on request performance, error rates, and credential health.

The metrics system MUST be implemented as a standalone module (`src/common/metrics.rs` or `src/kiro/metrics.rs`) that other modules instrument via lightweight calls. The ring buffer approach from dev-source is appropriate: bounded memory, no external dependencies (no Prometheus server required), queryable via Admin API.

Key instrumentation points:
1. Request lifecycle: total latency, TTFB, streaming duration
2. Credential distribution: requests per credential, failure rates per credential
3. Cache performance: hit/miss ratio from F-001
4. Error classification: upstream errors by type (from F-003)
5. Compression activation: how often compression triggers, compression ratios

## Interface Contract

- **MetricsCollector trait**: record_request(latency, ttfb, credential_id, status, model)
- **Admin API endpoints**: GET /admin/metrics (summary), GET /admin/metrics/window?duration=1h (windowed)
- **Consumers**: Admin UI dashboard, Admin API, internal diagnostics
- **Data retention**: Ring buffer with configurable window (default 1 hour, max 24 hours)

## Constraints (RFC 2119)

- MUST implement ring buffer with bounded memory (no unbounded growth)
- MUST track: request count, latency percentiles (p50/p95/p99), TTFB, error rate, credential distribution
- MUST expose metrics via Admin API (JSON format, Prometheus-compatible SHOULD be a stretch goal)
- MUST NOT introduce external dependencies (no StatsD/Prometheus client libraries in P0)
- SHOULD support per-model breakdown (requests and latency by model)
- MAY support real-time streaming metrics via SSE to Admin UI

## Test Approach

- Unit tests for ring buffer insertion, eviction, and windowed aggregation
- Unit tests for percentile calculation accuracy
- Integration test: simulate N requests, verify Admin API returns correct aggregated stats
- Stress test: verify metrics recording does not measurably impact request latency

## TODOs

- Study dev-source metrics.rs ring buffer implementation details
- Define Admin UI metrics dashboard wireframe (coordinate with UI role)
- Evaluate whether to expose compression pipeline metrics (activation frequency, bytes saved)
- Determine if metrics should persist across restarts (likely not for P0)
