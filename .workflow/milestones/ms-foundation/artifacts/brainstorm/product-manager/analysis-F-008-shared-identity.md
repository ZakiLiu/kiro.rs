# F-008 — CredentialIdentity Shared Trait

> Role: product-manager | Related decisions: PM-01, SA-04, SME-01

## Architecture

The CredentialIdentity shared trait is the foundational abstraction that enables F-001 (cross-request cache) without compromising the anti-detection system. Currently, fingerprint generation (fingerprint.rs) serves only anti-detection (affinity.rs). The cache system needs fingerprint-derived identifiers for cache keys, but sharing raw fingerprints would create detectable correlation between cache behavior and anti-detection identity.

Product perspective: This is infrastructure-level work with no direct user visibility, but it is a hard P0 prerequisite for F-001. Without CredentialIdentity, the cache system would either duplicate fingerprint logic (violating DRY and creating drift risk) or directly consume raw fingerprints (creating security risk per SA-04).

The trait MUST provide a derivation mechanism that allows each consumer (cache, affinity) to produce independent identifiers from the same credential, preventing upstream correlation.

## Interface Contract

- **Trait definition**: CredentialIdentity with methods for deriving context-specific identifiers
- **Consumers**: prompt_cache (F-001) uses cache_identity(), affinity uses affinity_identity()
- **Implementor**: Each credential produces a CredentialIdentity from its token/fingerprint data
- **Independence guarantee**: cache_identity() and affinity_identity() outputs MUST NOT be derivable from each other

## Constraints (RFC 2119)

- MUST define a shared trait that both cache and affinity modules depend on
- MUST ensure derived identifiers are cryptographically independent (e.g., HKDF with different info strings)
- MUST NOT expose raw fingerprint material through the trait interface
- MUST be implemented before F-001 can begin integration
- SHOULD be designed for extensibility (future consumers can derive new identity types without modifying the trait)

## Test Approach

- Unit tests for identity derivation: same credential produces stable identities, different credentials produce different identities
- Property tests: cache_identity and affinity_identity from same credential are uncorrelated
- Integration test with F-001: cache lookups use CredentialIdentity-derived keys correctly

## TODOs

- Study current fingerprint.rs implementation to understand available entropy sources
- Evaluate HKDF vs HMAC-SHA256 for identity derivation (HKDF preferred for domain separation)
- Define trait API surface with SA role
- Coordinate implementation timeline: F-008 MUST complete before F-001 integration begins
