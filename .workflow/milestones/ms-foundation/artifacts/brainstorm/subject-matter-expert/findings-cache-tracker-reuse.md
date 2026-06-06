# Finding: Existing CacheTracker is More Sophisticated Than Dev-Source's prompt_cache

> Role: subject-matter-expert | Impact: MEDIUM

## Description

A detailed comparison reveals that the current project's `CacheTracker` (`cache_tracker.rs`) is significantly more sophisticated than the dev-source's `prompt_cache.rs` in per-request prefix simulation:

1. **Rolling SHA-256 prefix fingerprinting**: The current project computes cumulative prefix hashes across tool definitions, system messages, and conversation messages. The dev-source uses a simpler hash of the full content block.
2. **TTL bucket awareness**: The current project distinguishes between 5-minute and 1-hour cache TTLs based on `cache_control` annotations, matching Anthropic's real prompt caching behavior. The dev-source uses a flat TTL.
3. **Minimum cacheable tokens**: The current project enforces model-specific thresholds (1024 for Sonnet, 2048 for Haiku-3, 4096 for Opus) that match Anthropic's documented minimums. The dev-source does not enforce this.
4. **Billing header canonicalization**: The current project normalizes `x-anthropic-billing-header` system blocks to a stable placeholder, preventing billing metadata drift from invalidating cache fingerprints.
5. **No false TTL refresh**: The current project correctly avoids refreshing `expires_at` on cache hits, matching Anthropic's write-once-TTL semantics.

The dev-source's advantage is solely in the cross-request dimension (conversation_id reuse), which the current project lacks.

## Affected Features

- F-001 (Cross-Request Cache): The implementation MUST build atop the existing `CacheTracker`, not replace it.
- F-002 (Request Metrics): Cache hit/miss metrics MUST use `CacheTracker`'s computed values, not independent estimation.

## Recommendation

When implementing F-001, the `CrossRequestCache` MUST be a separate layer that consumes `CacheTracker::build_profile()` output. The `CacheTracker` MUST NOT be modified or replaced — it is the authoritative source of per-request prefix fingerprints. The cross-request cache adds only the conversation_id mapping dimension on top.
