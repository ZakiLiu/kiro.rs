# F-002 — Request Metrics and Observability

> Role: test-strategist | Related decisions: TS-01, TS-04

## Architecture

The metrics module test suite MUST validate the ring buffer storage, windowed aggregation, and concurrent write safety. Dev-source uses a ring buffer approach (see design-research: "Ring buffer, TTFB tracking, stream abort detection") which is well-suited to deterministic unit testing.

Key testable components:
- **Ring buffer overflow** — Oldest entries are discarded when buffer is full.
- **Windowed aggregation** — Statistics (p50, p99, avg) computed over configurable time windows.
- **TTFB tracking** — Time-to-first-byte recorded per request.
- **Per-model breakdown** — Metrics segmented by model ID.
- **Error classification counters** — Counts by error type (upstream, auth, rate limit).
- **Stream abort detection** — Incomplete streams counted separately from successful completions.

## Interface Contract

- `MetricsCollector::record(event: RequestEvent)` — MUST be non-blocking; recording SHOULD NOT add latency to the request path.
- `MetricsCollector::query(window: Duration) -> MetricsSnapshot` — MUST only include events within the specified window.
- `MetricsSnapshot` — MUST contain: request_count, error_count, avg_latency_ms, p50_latency_ms, p99_latency_ms, ttfb_avg_ms, cache_hit_rate, per_model_counts.

Test doubles: Metrics MUST be testable without live requests. Create `RequestEvent` fixtures with known latencies and timestamps.

## Constraints (RFC 2119)

- Ring buffer MUST discard oldest entries on overflow without blocking writers.
- Aggregation MUST handle empty windows gracefully (return zero counts, not errors).
- Concurrent recording from multiple Tokio tasks MUST NOT cause data races.
- Percentile calculations MUST use at least linear interpolation (not just nearest rank).
- Metrics collection MUST NOT allocate on the hot path after initial buffer creation.

## Test Approach

**Unit tests (≥ 12 tests):**
1. Single event recording and retrieval.
2. Ring buffer overflow — insert N+1 events into buffer of size N, verify oldest is gone.
3. Windowed query — events outside window are excluded.
4. Empty window returns zero counts.
5. Percentile calculation with known data (e.g., 100 events with latencies 1..100ms).
6. Per-model breakdown — 3 models, verify counts are isolated.
7. Error classification — verify error_count increments for each error type.
8. TTFB tracking — verify average and p99 with controlled input.
9. Stream abort vs completion — separate counters.
10. Concurrent writes — 50 tasks recording simultaneously, verify total count.
11. Cache hit rate calculation — verify formula (hits / total).
12. Credential distribution — verify per-credential request counts.

**Async tests (tokio::test):**
- Spawn 50 concurrent record tasks, verify no panic and total count matches.
- Record events with `tokio::time::advance()`, verify windowed query correctness.

## TODOs

- Determine ring buffer capacity from design research or performance requirements.
- Decide whether metrics are Prometheus-compatible (affects snapshot format).
- Evaluate whether `parking_lot::Mutex` or `AtomicU64` counters are preferred for hot-path recording.
