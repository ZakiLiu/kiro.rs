---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-002
priority: must
wave: P0
---

# REQ-002: Request Metrics and Observability

## User Story

As a **Platform Operator**, I want to see real-time request latency, cache hit rates, credential distribution, and error classification through the Admin API, so that I can monitor proxy health, identify bottlenecks, and make informed capacity decisions.

## Description

The current kiro.rs has zero operational observability — no latency tracking, no error distribution, no cache effectiveness measurement. This is the most critical gap identified in the brainstorm phase. The metrics system provides a lightweight, in-process ring buffer that captures per-request signals without external dependencies (no Prometheus, no StatsD).

The `MetricsCollector` records each request as a `MetricRecord` in a fixed-size ring buffer (default 10,000 entries). When the buffer is full, the oldest entry is overwritten. The Admin API exposes aggregation endpoints that compute windowed statistics (latency percentiles, cache hit rate, error class distribution, credential usage balance) over configurable time windows.

The design avoids unbounded memory allocation and external dependencies. All recording operations MUST complete in constant time. The ring buffer uses `parking_lot::Mutex` consistent with existing codebase patterns (see `affinity.rs`, `token_manager.rs`). No I/O occurs inside the lock — recording is a simple struct copy into the buffer slot.

## Acceptance Criteria

1. **Recording**: Every completed request MUST produce a `MetricRecord` containing: timestamp, latency_ms, ttfb_ms (streaming only), credential_id, model, HTTP status, cache_hit (from REQ-001), and error_class (from REQ-003). The `record()` operation MUST complete in <500ns under concurrent load.

2. **Aggregation**: The Admin API MUST expose `GET /admin/metrics?window=300` returning a `MetricsSnapshot` with: request_count, p50/p95/p99 latency, mean TTFB, cache hit rate, per-credential request distribution, and per-error-class counts. Windowed stats MUST support configurable window sizes (default 300 seconds).

3. **Ring Buffer Bounds**: The ring buffer size MUST be configurable (default: 10,000, range: 100..1,000,000). The system MUST NOT allocate memory beyond the configured buffer size for metrics storage.

4. **Graceful Degradation**: If metrics are disabled via configuration, the system MUST NOT incur any recording overhead. The `record()` call SHOULD be a no-op when disabled.

## Configuration

```json
{
  "metrics": {
    "enabled": true,
    "ringBufferSize": 10000,
    "defaultWindowSeconds": 300
  }
}
```

## Signals Tracked

| # | Signal | Type | Source | Aggregation |
|---|--------|------|--------|-------------|
| 1 | request_latency_ms | histogram | `handlers.rs` — total request duration | p50, p95, p99 |
| 2 | ttfb_ms | histogram | `stream.rs` — time to first byte | p50, p95, p99 |
| 3 | cache_hit_rate | gauge | CrossRequestCache — hits / (hits + misses) | ratio per window |
| 4 | credential_usage | counter/credential | `provider.rs` — requests per credential | distribution |
| 5 | error_class_count | counter/class | ErrorMapper — classified errors | count per class |
| 6 | compression_ratio | gauge | `compressor.rs` — original / compressed | mean per window |
| 7 | active_cache_entries | gauge | CrossRequestCache — current count | point-in-time |
| 8 | stream_abort_count | counter | `stream.rs` — client disconnections | count per window |

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-001 | **Soft** — Records cache hit/miss events from cross-request cache |
| REQ-003 | **Soft** — Records error classification from ErrorMapper |
| REQ-008 | **None** — No direct dependency |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-07 | System Architect | Ring-buffer design, Admin API exposure |
| PM-07 | Product Manager | Most critical operational gap |
| SME-07 | Subject Matter Expert | Five domain signals that must be tracked |
| TS-04 | Test Strategist | Overflow and concurrent access testing |

## Admin API Endpoint

```
GET /admin/metrics?window={seconds}

Response:
{
  "window_seconds": 300,
  "request_count": 1523,
  "latency": { "p50": 120, "p95": 450, "p99": 890 },
  "ttfb": { "p50": 45, "p95": 180, "p99": 350 },
  "cache": { "hits": 892, "misses": 631, "hit_rate": 0.586 },
  "credentials": { "1": 800, "2": 723 },
  "errors": { "rate_limit": 12, "bad_request": 3, "server_error": 1 },
  "active_cache_entries": 456
}
```

## Health Check Enhancement

`GET /health` SHOULD return `credentials_available` count and `cache_entries` count alongside existing status.
