# TASK-001: Implement CredentialIdentity trait on KiroCredentials

## Changes
- `src/kiro/identity.rs`: Created new module with `CredentialIdentity` trait (3 methods) + impl for `KiroCredentials` + 5 unit tests
- `src/kiro/mod.rs`: Added `pub mod identity;` registration

## Verification
- [x] File src/kiro/identity.rs exists and defines pub trait CredentialIdentity: confirmed via file creation
- [x] impl CredentialIdentity for KiroCredentials exists: confirmed in identity.rs
- [x] cache_identity() uses SHA-256 with "cache:" domain prefix: uses `Sha256::new()` + `update(b"cache:")` + `update(seed)`
- [x] detection_identity() reuses Fingerprint::generate_from_seed: delegates directly to `Fingerprint::generate_from_seed(seed)`
- [x] credential_id() returns self.id.unwrap_or(0): confirmed
- [x] src/kiro/mod.rs contains "pub mod identity;": confirmed
- [x] cargo test passes with >= 3 unit tests: 5 tests pass (determinism, uniqueness, domain_separation, credential_id_none, seed_fallback_chain)

## Tests
- [x] `cargo test -- identity`: 5 passed, 0 failed
- [x] `cargo clippy`: no identity-related warnings (pre-existing warnings in other files outside scope)

## Deviations
- Trait returns owned `Fingerprint` instead of `&Fingerprint` as originally specified in action field. The constraints section explicitly required this: "Return OWNED Fingerprint (not &Fingerprint) since KiroCredentials doesn't store it." This avoids needing OnceCell/lazy field on the data-only KiroCredentials struct.

## Notes
- The `credential_seed()` helper is a standalone function (not a method) to keep KiroCredentials as pure data model
- Seed fallback chain: refresh_token > kiro_api_key > machine_id > "unknown"
- `#![allow(dead_code)]` added to module since the trait is not consumed yet (foundation for downstream tasks)
- Pre-existing uncommitted changes exist in main.rs, handlers.rs, router.rs, middleware.rs, config.rs (metrics module partially added) — these are outside task scope
