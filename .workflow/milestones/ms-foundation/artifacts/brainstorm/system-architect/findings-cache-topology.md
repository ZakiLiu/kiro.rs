# Finding: Cache Topology -- Two Independent Cache Layers

> Role: system-architect | Impact: HIGH

## Description

The current kiro.rs has one cache layer: CacheTracker in `anthropic/cache_tracker.rs`, which simulates Anthropic prompt cache behavior for usage accounting (estimating cache_read_input_tokens and cache_creation_input_tokens). The proposed F-001 CrossRequestCache introduces a second, architecturally distinct cache layer that manages Kiro conversation_id reuse across requests.

These two caches serve completely different purposes and operate at different points in the request lifecycle:

- **CacheTracker** (existing): Post-conversion, pre-response. Builds a SHA-256 prefix fingerprint of the request content, tracks which prefixes have been seen per credential, and estimates cache token usage. This is a simulation layer -- it does not affect the actual request sent to upstream.

- **CrossRequestCache** (proposed): Pre-conversion. Looks up a cached conversation_id based on (credential_id, derived_cache_key) and injects it into the Kiro request. This actively modifies the upstream request to enable server-side cache reuse.

The architectural risk is that these two layers could interfere: if CrossRequestCache changes the upstream request shape (by injecting conversation_id), does CacheTracker need to account for this in its prefix fingerprint calculation. The answer is no -- CacheTracker operates on the Anthropic-format request before conversion, while conversation_id is injected during Kiro conversion. The two layers are orthogonal.

## Affected Features

- F-001 cross-request-cache (directly -- this is the new cache layer)
- F-002 request-metrics (cache_hit_rate metric must clarify which cache it refers to)
- F-008 shared-identity (provides the cache key for CrossRequestCache)
- Existing CacheTracker (must remain unmodified)

## Recommendation

Maintain strict separation between the two cache layers. Name them distinctly in code and metrics to avoid confusion. CacheTracker remains in `anthropic/` (Anthropic-format concern). CrossRequestCache belongs in `kiro/` (Kiro-protocol concern). Metrics SHOULD distinguish `prefix_cache_hit` (CacheTracker estimation) from `conversation_cache_hit` (CrossRequestCache lookup). Documentation MUST clarify that CacheTracker is a simulation for billing estimation while CrossRequestCache is an optimization that affects upstream behavior.
