# F-008 -- CredentialIdentity Shared Trait

> Role: system-architect | Related decisions: SA-04, SME-01, SA-02

## Architecture

A new trait `CredentialIdentity` in `src/kiro/identity.rs` provides a unified abstraction over the fingerprint system. Currently, the Fingerprint struct in `kiro/fingerprint.rs` generates deterministic device fingerprints from a seed (typically refresh_token). The affinity system in `kiro/affinity.rs` uses credential_id (u64) for user-credential binding. The cross-request cache (F-001) needs a stable key derived from the fingerprint.

**The problem:** Cache keys must be stable per-credential but MUST NOT leak the raw fingerprint to the cache layer (security boundary). Affinity uses a simple credential_id. The CredentialIdentity trait provides both: an identity_key for general identification and a derived_cache_key that hashes the fingerprint with a domain separator for cache use.

**Module placement:** `src/kiro/identity.rs` as a new file in the kiro module, since both Fingerprint and credential management live in kiro/.

**Implementation on existing types:** The trait is implemented for a new CredentialContext struct that wraps (credential_id, Fingerprint). This avoids modifying the existing Fingerprint struct while providing the unified interface.

**Dependency graph:**
- F-008 (this) is a prerequisite for F-001 (cross-request-cache)
- F-008 uses existing fingerprint.rs (no modification needed)
- affinity.rs MAY be refactored to use CredentialIdentity instead of raw credential_id (optional, not required)

## Interface Contract

> **Cross-Role Resolution (C-001)**: Adopt SME's three-method signature (detection_identity, cache_identity, credential_id) — it provides proper domain separation and preserves existing credential_id usage.

<!-- superseded by C-001 -->
```rust
pub trait CredentialIdentity {
    /// Stable identifier for this credential (typically credential_id as bytes)
    fn identity_key(&self) -> [u8; 32];
    
    /// Derived key for cache operations -- domain-separated hash of fingerprint
    /// MUST NOT be reversible to the original fingerprint
    fn derived_cache_key(&self) -> [u8; 32];
}
```
<!-- /superseded -->

pub struct CredentialContext {
    pub credential_id: u64,
    pub fingerprint: Fingerprint,
}

impl CredentialIdentity for CredentialContext {
    fn identity_key(&self) -> [u8; 32] {
        // SHA256(b"identity:" || credential_id.to_be_bytes())
        sha256(&[b"identity:", &self.credential_id.to_be_bytes()])
    }
    
    fn derived_cache_key(&self) -> [u8; 32] {
        // SHA256(b"cache:" || credential_id.to_be_bytes() || machine_id.as_bytes())
        sha256(&[b"cache:", &self.credential_id.to_be_bytes(), self.fingerprint.machine_id.as_bytes()])
    }
}
```

The domain separator prefix ("identity:", "cache:") prevents key collision between different uses of the same underlying data. The derived_cache_key incorporates the machine_id from the fingerprint, which is deterministic per credential (generated from refresh_token seed).

## Constraints (RFC 2119)

- MUST define a trait with identity_key() and derived_cache_key() methods returning [u8; 32]
- MUST use domain-separated hashing to prevent cross-purpose key reuse
- MUST NOT expose raw fingerprint data through the trait interface
- MUST produce deterministic output for the same credential (same seed -> same keys)
- SHOULD use SHA-256 consistent with existing fingerprint.rs hashing (sha2 crate already in deps)
- MUST NOT modify the existing Fingerprint struct or affinity.rs public API
- SHOULD provide CredentialContext construction from existing credential data without additional I/O

## Test Approach

- **Unit tests:** Determinism -- same credential_id + fingerprint always produces same identity_key and derived_cache_key. Different credentials produce different keys. Domain separation -- identity_key != derived_cache_key for same input.
- **Property tests:** No collisions in 10000 randomly-seeded credentials (statistical check).
- **Integration tests:** CredentialContext correctly constructed from MultiTokenManager credential data. CrossRequestCache uses derived_cache_key for lookups.
- **Security tests:** derived_cache_key is not reversible to fingerprint (by construction -- SHA-256 is one-way).

## TODOs

- Determine where CredentialContext is constructed in the request flow (provider.rs during credential selection is the natural point)
- Evaluate whether affinity.rs should adopt CredentialIdentity or remain on raw credential_id (simpler, lower risk)
- Decide if the trait needs an associated type or if [u8; 32] is sufficient for all use cases
- Consider whether rate_limiter.rs should also use CredentialIdentity (currently uses credential_id directly)
