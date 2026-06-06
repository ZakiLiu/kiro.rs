---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-008
priority: must
wave: P0
---

# REQ-008: CredentialIdentity Shared Trait

## User Story

As a **system architect**, I want a unified trait that provides domain-separated identity derivation for each credential, so that the anti-detection fingerprint and the cache key are cryptographically independent — preventing upstream correlation between a credential's simulated device identity and its cache access pattern.

## Description

The current kiro.rs uses `Fingerprint` (in `fingerprint.rs`) to generate simulated device identities from a seed value (`refresh_token` or `machine_id`). This fingerprint is used by `affinity.rs` for credential binding and by the request pipeline for User-Agent and header generation. The new cross-request cache (REQ-001) also needs a credential-derived key — but it MUST NOT use the same fingerprint, because correlating cache access patterns with device fingerprints would allow upstream detection systems to link "different users" who share a cache partition.

The `CredentialIdentity` trait provides three methods with strict domain separation:

1. **`detection_identity() -> &Fingerprint`**: Returns the existing device fingerprint used for anti-detection headers (User-Agent, Accept-Language, screen resolution, etc.). This is the "who this credential pretends to be" identity. Consumers: `affinity.rs`, request header generation, `rate_limiter.rs`.

2. **`cache_identity() -> [u8; 32]`**: Returns a 32-byte key derived from the same seed but using domain-separated SHA-256 (`SHA-256("kiro.cache:" || seed)`). This key is used exclusively for cross-request cache lookups. It MUST NOT be derivable from `detection_identity()` and vice versa. Consumer: `CrossRequestCache` (REQ-001).

3. **`credential_id() -> u64`**: Returns a stable numeric identifier for the credential, used for bulk operations (cache invalidation on cooldown, metrics per-credential counts). This is a simple hash, not security-critical.

The domain separation is achieved by prefixing the SHA-256 input with different domain strings before hashing. Given the same seed `S`:
- Detection: `SHA-256("kiro.detect:" || S)` → selects fingerprint parameters
- Cache: `SHA-256("kiro.cache:" || S)` → cache key
- ID: lower 8 bytes of `SHA-256("kiro.id:" || S)` → credential_id

This ensures that even with knowledge of one identity, the others cannot be derived without the original seed.

## Acceptance Criteria

1. **Trait Definition**: A `CredentialIdentity` trait MUST be defined with three methods: `detection_identity() -> &Fingerprint`, `cache_identity() -> [u8; 32]`, and `credential_id() -> u64`. The trait MUST be implemented on the credential struct used by `TokenManager`.

2. **Domain Separation**: `cache_identity()` and `detection_identity()` MUST use different SHA-256 domain prefixes. Given the same seed, it MUST be computationally infeasible to derive one from the other without the seed. Unit tests MUST verify that identical seeds produce different outputs for each method.

3. **Backward Compatibility**: The existing `Fingerprint::generate_from_seed()` behavior MUST NOT change. The `detection_identity()` method MUST return the same fingerprint that the credential currently produces. All existing anti-detection behavior MUST be preserved.

4. **Consumer Migration**: `affinity.rs` MUST use `detection_identity()` instead of direct fingerprint access. `CrossRequestCache` (REQ-001) MUST use `cache_identity()`. `rate_limiter.rs` MUST continue using `detection_identity()` for rate limit key derivation.

## Interface Contract

```rust
pub struct CacheKey(pub [u8; 32]);
pub struct CredentialId(pub u64);

pub trait CredentialIdentity {
    /// Anti-detection device fingerprint — used for headers, affinity, rate limiting.
    /// MUST NOT be used for cache key derivation.
    fn detection_identity(&self) -> &Fingerprint;

    /// Domain-separated cache key — used exclusively for CrossRequestCache.
    /// MUST NOT correlate with detection_identity().
    fn cache_identity(&self) -> CacheKey;

    /// Stable numeric credential identifier — used for bulk operations.
    fn credential_id(&self) -> CredentialId;
}
```

## Domain Separation Derivation

```
Seed S (refresh_token or machine_id)

detection_identity:
  hash = SHA-256("kiro.detect:" || S)
  fingerprint = Fingerprint::generate_from_hash(hash)

cache_identity:
  CacheKey = SHA-256("kiro.cache:" || S)

credential_id:
  full_hash = SHA-256("kiro.id:" || S)
  CredentialId = u64::from_le_bytes(full_hash[0..8])
```

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-001 | **Depended on by** — Cache uses `cache_identity()` for key derivation |
| REQ-004 | **Depended on by** — Converter uses cached conversation_id looked up via cache_identity |
| REQ-002 | **Soft** — Metrics uses `credential_id()` for per-credential counters |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-04 | System Architect | CredentialIdentity trait with identity_key() and derived_cache_key() |
| PM-04 | Product Manager | P0 prerequisite — hard dependency for F-001 and F-004 |
| SME-01 | Subject Matter Expert | Domain-separated SHA-256 derivation |
| SME-04 | Subject Matter Expert | Anti-detection is the core competitive moat |
| TS-10 | Test Strategist | Cross-module consistency — same input, same fingerprint |
| C-003 | Constraint (locked) | MUST implement domain-separated identities |

## Migration Plan

1. Define `CredentialIdentity` trait in a new file (or `token_manager/identity.rs` per REQ-007)
2. Implement trait on existing credential struct, wrapping current `Fingerprint::generate_from_seed()`
3. Add `cache_identity()` and `credential_id()` with domain-separated derivation
4. Migrate `affinity.rs` to use `detection_identity()` through the trait
5. Wire `cache_identity()` into `CrossRequestCache` (REQ-001)
6. Verify all existing tests pass (behavioral equivalence for detection path)

## Security Properties

- **Non-correlation**: An observer who intercepts both cache access patterns and HTTP fingerprint headers MUST NOT be able to determine they belong to the same credential.
- **Determinism**: The same seed MUST always produce the same identities. Random per-request generation would be detectable as non-human behavior.
- **Seed confidentiality**: The seed (`refresh_token` / `machine_id`) is the secret. Neither identity leaks the seed. Both identities are one-way derivations.
