---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: NFR-PERF-001
priority: must
category: performance
---

# NFR-PERF-001: Latency Overhead Budget

## User Story

As a **Power User**, I want the new cache lookup and metrics recording to add no more than 5ms of overhead to each request, so that the proxy remains a negligible latency layer between my client and the upstream API.

## Description

The fusion introduces two new operations in the hot request path: cross-request cache lookup (REQ-001) and metrics recording (REQ-002). Both are in-memory operations with no I/O, but they introduce locking overhead (parking_lot::Mutex) and computational cost (SHA-256 hashing for cache key, struct copy for metrics recording).

This NFR establishes a strict latency budget: the combined overhead of all new operations MUST NOT exceed 5ms at p99 under production-representative load. Individual operation budgets are allocated as follows:

- **Cache lookup**: <1ms (HashMap lookup under Mutex, no allocation)
- **Cache insert**: <1ms (HashMap insert under Mutex, possible LRU eviction)
- **Metrics record()**: <500ns (ring buffer slot write under Mutex)
- **SHA-256 cache key derivation**: <100μs (computed once per request, amortized)
- **Tool name shortening**: <500μs per tool (SHA-256 + HashMap insert, typically <10 tools)

The budget assumes the Mutex critical sections contain no I/O and are kept to the minimum necessary operations (read/write a HashMap slot, increment a buffer index). Any lock contention under concurrent load MUST be resolved by reducing critical section scope, not by increasing the budget.

## Acceptance Criteria

1. **Combined Overhead**: The total latency added by cache lookup, cache insert, metrics recording, and tool name shortening MUST be <5ms at p99 under 100 concurrent requests. This MUST be validated via benchmark tests before merging P0 features.

2. **Record Latency**: `MetricsCollector::record()` MUST complete in <500ns on average. This is measured as the wall-clock time from entering `record()` to returning, including Mutex acquisition. Benchmark tests MUST validate this under 16-thread contention.

3. **No Allocation in Hot Path**: Cache lookup and metrics recording MUST NOT allocate heap memory on the common path (cache hit, ring buffer not full). Allocation is acceptable only for cache miss + insert (new entry) and ring buffer resize (configuration change).

4. **Degradation Under Load**: Under extreme load (>1000 concurrent requests), the system SHOULD degrade gracefully. If Mutex contention exceeds acceptable thresholds, the system SHOULD skip metrics recording rather than blocking the request. Cache lookup failure SHOULD result in a cache miss (proceed without conversation_id), not a request failure.

## Measurement Method

```rust
// Benchmark template using criterion
fn bench_cache_lookup(c: &mut Criterion) {
    let cache = CrossRequestCache::new(config);
    // Pre-populate with 500 entries
    c.bench_function("cache_lookup_hit", |b| {
        b.iter(|| cache.lookup(&known_key))
    });
}

fn bench_metrics_record(c: &mut Criterion) {
    let collector = MetricsCollector::new(config);
    c.bench_function("metrics_record", |b| {
        b.iter(|| collector.record(sample_metric()))
    });
}
```

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-001 | Cache operations contribute to latency budget |
| REQ-002 | Metrics recording contributes to latency budget |
| REQ-004 | Tool name shortening contributes to latency budget |
| REQ-008 | SHA-256 key derivation contributes to latency budget |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-02 | System Architect | In-memory HashMap for cache, no network calls |
| SA-07 | System Architect | Ring buffer for constant-time metrics recording |
| PM-06 | Product Manager | <5ms overhead target in success metrics |
| SME-07 | Subject Matter Expert | record() latency target <500ns |

## Risk Mitigation

| Risk | Probability | Mitigation |
|------|------------|------------|
| Mutex contention under high concurrency | Medium | parking_lot::Mutex is optimized for short critical sections; benchmark validates under load |
| SHA-256 computation cost | Low | Computed once per request, cached in request context |
| LRU eviction cascade | Low | Eviction is O(1) with doubly-linked list; bounded by max_entries |
| Tool name shortening with many tools | Low | SHA-256 is fast; 100 tools × 500μs = 50ms would trigger investigation |
