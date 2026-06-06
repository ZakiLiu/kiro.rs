---
document: architecture
session_id: BLP-kiro-fusion-2026-06-05
adr_id: ADR-001
title: CredentialIdentity Trait Design
status: proposed
created: 2026-06-05
deciders:
  - fusion-blueprint-generator
traces:
  - brainstorm/decisions.md#credential-identity
---

# ADR-001: CredentialIdentity Trait Design

## Context

The fusion blueprint merges two codebases with overlapping credential management needs:

1. **Anti-detection** (dev-source): Each credential MUST present a unique browser-like
   fingerprint to upstream APIs. This identity drives request header diversification,
   TLS fingerprint selection, and behavioral shaping.

2. **Cross-request caching** (fusion target): Cached conversation IDs MUST be scoped
   to the credential that created them, because upstream sessions are credential-bound.

These two concerns require identity derivation from the same credential data, but the
derived identities MUST NOT be correlated — leaking the cache identity SHOULD NOT
reveal the detection fingerprint, and vice versa.

The current codebase has `kiro/fingerprint.rs` for basic credential identification,
but it serves a single purpose and lacks domain separation.

## Decision

We MUST introduce a `CredentialIdentity` trait with three methods:

```rust
pub trait CredentialIdentity {
    /// Identity for anti-detection systems (header diversification, TLS profile).
    /// Domain prefix: "detection:"
    fn detection_identity(&self) -> IdentityHash;

    /// Identity for cross-request cache scoping.
    /// Domain prefix: "cache:"
    fn cache_identity(&self) -> IdentityHash;

    /// Human-readable credential identifier for logging and admin UI.
    fn credential_id(&self) -> &str;
}
```

**Key design properties:**

- **Domain-separated SHA-256**: Each method computes `SHA-256(domain_prefix || salt || credential_material)`.
  The domain prefix ensures cryptographic independence — knowing one hash reveals nothing about the other.
- **Salt**: A configurable per-deployment salt (`fingerprint.salt` in config) prevents rainbow table attacks.
- **IdentityHash**: A newtype wrapper around `[u8; 32]` with `Display` (hex), `Eq`, `Hash` implementations.
  No `Serialize` — identities MUST NOT appear in API responses.
- **Implementation site**: `kiro/model/credentials.rs`, implemented for the existing credential struct.

## Alternatives Considered

### (a) Two-Method SA Approach

The dev-source uses a two-method design (`fingerprint()` + `credential_id()`). This conflates
detection identity with general-purpose fingerprinting, making it unsafe to reuse for cache
scoping without domain separation.

**Rejected**: Insufficient separation for the fusion's dual-use requirement.

### (b) Shared Raw Fingerprint

Derive a single raw fingerprint and let consumers hash it with their own prefix.

**Rejected**: Pushes domain-separation responsibility to every call site, violating
DRY and increasing the risk of a consumer forgetting to add a prefix.

### (c) Separate Traits

`DetectionIdentity` and `CacheIdentity` as independent traits.

**Rejected**: Over-engineering for the current scope. The shared `credential_id()` method
naturally belongs with both identities. A single trait with three methods is simpler and
enforces co-implementation.

## Consequences

**Positive:**
- Cryptographic independence between detection and cache domains.
- Single implementation point; consumers call the appropriate method without
  worrying about hashing details.
- `credential_id()` provides a safe, human-readable label for logs and admin UI
  without exposing sensitive material.

**Negative:**
- Adds a trait boundary that all credential types MUST implement.
- SHA-256 computation on every call; SHOULD cache results if profiling shows overhead
  (unlikely — hashing is ~100ns per call).

**Risks:**
- If a future domain requires a third identity type, the trait grows. Mitigated by
  the domain-prefix pattern — adding a method is additive, not breaking.

## Implementation Notes

- The `IdentityHash` newtype MUST implement `Eq` and `Hash` for use as HashMap keys.
- `Debug` impl MUST truncate to first 8 hex chars to prevent log leakage.
- Unit tests MUST verify that `detection_identity() != cache_identity()` for the same credential.
