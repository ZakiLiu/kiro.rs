# F-008 — CredentialIdentity Shared Trait

> Role: test-strategist | Related decisions: TS-01, TS-10

## Architecture

CredentialIdentity is a cross-cutting trait that unifies fingerprint usage between anti-detection (affinity.rs) and cache key generation (F-001). The existing fingerprint.rs module has 8 tests. The new trait MUST be tested for cross-module consistency and isolation guarantees.

Key testable components:
- **Trait contract** — Same credential input produces same identity output across calls.
- **Determinism** — Identity derivation MUST be deterministic (no random components unless explicitly seeded).
- **Isolation** — Cache key derivation and affinity fingerprint MAY produce different values from the same identity to prevent upstream correlation (see §9 Risks).
- **Backward compatibility** — Existing fingerprint.rs tests MUST continue to pass.

## Interface Contract

- `trait CredentialIdentity { fn identity_key(&self) -> IdentityKey; fn cache_key(&self) -> CacheKey; fn affinity_fingerprint(&self) -> AffinityFingerprint; }`
- `IdentityKey` — Common base; `CacheKey` and `AffinityFingerprint` MUST be derived from it but MAY diverge.
- Implementors: credential structs in token_manager.rs.

## Constraints (RFC 2119)

- `identity_key()` MUST be deterministic for the same credential across process restarts.
- `cache_key()` and `affinity_fingerprint()` MUST be derivable from `identity_key()` without additional I/O.
- `cache_key()` and `affinity_fingerprint()` SHOULD produce different values to prevent upstream correlation (see §9 fingerprint reuse risk).
- The trait MUST NOT require async — it is a pure computation.
- Existing fingerprint.rs tests (8 tests) MUST pass without modification after trait extraction.

## Test Approach

**Unit tests (≥ 10 tests):**
1. Same credential produces same identity_key — determinism.
2. Different credentials produce different identity_keys — uniqueness.
3. cache_key derived from identity_key — consistent derivation.
4. affinity_fingerprint derived from identity_key — consistent derivation.
5. cache_key differs from affinity_fingerprint — isolation guarantee.
6. Determinism across "restarts" — serialize/deserialize credential, verify same keys.
7. Trait implementable on mock struct — verify trait is not over-constrained.
8. Empty credential fields — handled gracefully.
9. Special characters in credential — no key derivation failure.
10. Performance — key derivation completes in < 1ms for single credential.

**Integration tests:**
- Create credential, derive cache_key, use it to store/retrieve from cache (F-001). Verify the full chain works.
- Create credential, derive affinity_fingerprint, verify it matches existing fingerprint.rs behavior.

**Regression:**
- All 8 existing fingerprint.rs tests MUST pass. This is the primary regression gate for the trait extraction.

## TODOs

- Study existing fingerprint.rs derivation algorithm to determine if cache_key can be derived from the same seed.
- Determine whether `IdentityKey` should be a newtype wrapper or a raw `[u8; 32]`.
- Evaluate whether the trait should be in a new `identity.rs` file or merged into fingerprint.rs.
