# F-008 — CredentialIdentity Shared Trait

> Role: subject-matter-expert | Related decisions: SME-01, SA-04

## Architecture

> **Cross-Role Resolution (C-001)**: This trait signature is the agreed contract — SA's two-method variant is superseded.

The current project generates per-credential fingerprints in `fingerprint.rs` using `Fingerprint::generate_from_seed(refresh_token)`. This fingerprint is used by:
1. `affinity.rs` — user-to-credential binding for session continuity
2. `provider.rs` — User-Agent and x-amz-user-agent header construction
3. `cache_tracker.rs` — per-credential cache partitioning (keyed by `credential_id: u64`)

With F-001 introducing cross-request caching, the fingerprint becomes a shared dependency between anti-detection (identity simulation) and caching (content-addressing). These two concerns have conflicting requirements: anti-detection wants fingerprint diversity and opacity, while caching wants fingerprint stability and determinism.

The `CredentialIdentity` trait resolves this by providing two distinct derivations from the same seed:

```rust
pub trait CredentialIdentity {
    /// Identity for anti-detection: User-Agent, affinity binding
    fn detection_identity(&self) -> &Fingerprint;

    /// Identity for cache keying: deterministic, non-correlatable with detection_identity
    fn cache_identity(&self) -> [u8; 32];

    /// Stable credential identifier (existing credential_id)
    fn credential_id(&self) -> u64;
}
```

The `cache_identity()` derivation uses `SHA-256("cache:" + seed)` while `detection_identity()` uses the existing `Fingerprint::generate_from_seed(seed)`. The `"cache:"` domain separator ensures the two identities are cryptographically independent — knowing one does not reveal the other.

## Interface Contract

```rust
pub trait CredentialIdentity: Send + Sync {
    fn detection_identity(&self) -> &Fingerprint;
    fn cache_identity(&self) -> [u8; 32];
    fn credential_id(&self) -> u64;
}

// Implementation on existing CredentialState in token_manager.rs
impl CredentialIdentity for CredentialState {
    fn detection_identity(&self) -> &Fingerprint {
        &self.fingerprint
    }

    fn cache_identity(&self) -> [u8; 32] {
        sha256(format!("cache:{}", self.seed).as_bytes())
    }

    fn credential_id(&self) -> u64 {
        self.id
    }
}
```

Consumers: `kiro/provider.rs` (header construction via `detection_identity()`), `anthropic/cross_request_cache.rs` (cache keying via `cache_identity()`), `kiro/affinity.rs` (binding via `credential_id()`).

## Constraints (RFC 2119)

- The trait MUST provide cryptographically independent identities for detection and caching — domain-separated SHA-256 derivation satisfies this.
- The trait MUST be implemented on the existing `CredentialState` struct without breaking current fingerprint generation logic.
- The `cache_identity()` output MUST be deterministic for the same credential seed across process restarts.
- The trait MUST be `Send + Sync` to support `Arc<dyn CredentialIdentity>` usage in async contexts.
- The trait SHOULD be defined in a new `kiro/identity.rs` module, not in `fingerprint.rs`, to maintain single-responsibility.
- Existing code that accesses `Fingerprint` directly SHOULD be migrated to use the trait, but MAY remain as-is if the migration is non-trivial and the direct access is in a module that only needs detection identity.

## Test Approach

- Unit tests: Verify `detection_identity()` and `cache_identity()` produce different outputs for the same seed.
- Cryptographic independence: Verify that knowing `cache_identity()` does not allow recovering the detection fingerprint fields.
- Determinism: Same seed across multiple instantiations produces identical `cache_identity()` values.
- Trait object safety: Verify `Arc<dyn CredentialIdentity>` compiles and works across async boundaries.

## TODOs

- Audit all call sites of `Fingerprint::generate_from_seed()` and `credential.fingerprint` to plan migration order.
- Decide whether `CredentialIdentity` should also expose `user_agent()` and `x_amz_user_agent()` or leave those on `Fingerprint`.
- Evaluate whether the domain separator string `"cache:"` is sufficient or if a more structured HKDF derivation is warranted.
