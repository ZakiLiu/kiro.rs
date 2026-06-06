# TASK-003: Implement CrossRequestCache with LRU eviction and converter integration

## Changes
- `src/anthropic/cross_request_cache.rs`: Created new module with LRU cache (HashMap+VecDeque), lookup/insert/content_fingerprint methods, and 8 unit tests
- `src/anthropic/converter.rs`: Added `forced_conversation_id: Option<&str>` parameter to `convert_request()`; when Some, uses it directly instead of deriving from metadata/history fingerprint; updated all 16 test callers to pass `None`
- `src/anthropic/handlers.rs`: Added cache lookup before `convert_request()` (computes SHA-256 content fingerprint, queries cache); added cache insert after successful conversion (stores conversation_id); passes `forced_conversation_id` to converter
- `src/anthropic/middleware.rs`: Added `pub cross_request_cache: Option<Arc<CrossRequestCache>>` field to AppState; added `with_cross_request_cache()` builder method
- `src/anthropic/router.rs`: Added `cross_request_cache` parameter to `create_router_with_provider()`; wires it into AppState
- `src/main.rs`: Constructs `CrossRequestCache` from config and passes to router
- `src/model/config.rs`: Added `cross_request_cache_enabled: bool` (default true) and `cross_request_cache_max_entries: usize` (default 1000) with serde defaults
- `src/anthropic/mod.rs`: Added `pub(crate) mod cross_request_cache;`

## Verification
- [x] cross_request_cache.rs exists with pub struct CrossRequestCache: confirmed, uses Mutex<LruInner> with HashMap+VecDeque for LRU
- [x] lookup() returns Option<String>: confirmed, moves entry to LRU tail on hit
- [x] insert() stores and evicts LRU: confirmed, pops oldest from VecDeque when full
- [x] content_fingerprint() computes SHA-256: confirmed, hashes role + content of all messages
- [x] convert_request() has forced_conversation_id parameter: confirmed, Option<&str>
- [x] When forced_conversation_id is Some, converter uses it directly: confirmed, skips metadata/history derivation
- [x] handlers.rs performs cache lookup before convert and passes result: confirmed
- [x] handlers.rs inserts cache entry after successful conversion: confirmed
- [x] AppState has cross_request_cache field: confirmed, Option<Arc<CrossRequestCache>>
- [x] Config has cache config fields: confirmed, cross_request_cache_enabled (default true), cross_request_cache_max_entries (default 1000)
- [x] mod.rs declares cross_request_cache: confirmed, pub(crate) mod
- [x] cargo test passes with cache tests: confirmed, 8 tests covering hit/miss, LRU eviction, access refresh, credential isolation, fingerprint determinism, update existing

## Tests
- [x] `cargo test -- cross_request_cache`: 8 passed, 0 failed
- [x] `cargo test -- converter`: 56 passed, 0 failed
- [x] `cargo test`: 415 passed, 0 failed (full suite including TASK-001 and TASK-002 tests)
- [x] `cargo clippy`: 5 pre-existing warnings (not from this change), no new warnings
- [x] `cargo build`: succeeded

## Deviations
- Used HashMap+VecDeque for LRU instead of LinkedHashMap/IndexMap (no new crate dependency needed)
- Cache key uses credential_id=0 (global) instead of per-credential, because credential_id is not known until provider.call_api selects a credential, which happens after convert_request. The conversation_id should be consistent across credentials for the same content.
- Cache insert happens after conversion (before API call) rather than after response completion, since the conversation_id is already determined at conversion time and the goal is to reuse the same ID for subsequent identical requests

## Notes
- The clippy -D warnings failure is from 5 pre-existing warnings in handlers.rs and stream.rs (collapsible_match, unnecessary_cast), not introduced by this task
- The cache uses a global credential_id of 0 for simplicity. If per-credential isolation is needed in the future, the cache lookup/insert would need to be moved after credential selection in the provider layer.
