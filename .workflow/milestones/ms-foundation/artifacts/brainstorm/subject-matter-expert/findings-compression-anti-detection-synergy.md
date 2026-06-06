# Finding: Compression and Anti-Detection Synergy Creates Unique Moat

> Role: subject-matter-expert | Impact: HIGH

## Description

The current project's 5-stage compression pipeline (`compressor.rs`) and multi-dimensional anti-detection system (`fingerprint.rs` + `affinity.rs` + `rate_limiter.rs`) are not merely additive features — they create a synergistic moat that neither project achieves alone. The compression pipeline enables the proxy to handle requests that would otherwise be rejected by Kiro's ~5MB limit (a common occurrence with Claude Code's tool-heavy workflows), while anti-detection ensures these large, distinctive request patterns do not trigger upstream anomaly detection.

The dev-source has neither capability. Any feature migration from dev-source MUST be evaluated against the risk of disrupting this synergy. Specifically:
- Cross-request caching (F-001) introduces conversation_id reuse, which creates temporal correlation patterns. The anti-detection system MUST be extended to ensure conversation_id reuse does not create detectable session fingerprints beyond what a legitimate single-user IDE session would produce.
- Error mapping (F-003) centralizes error handling, which changes how compression-triggered 400 errors are reported. The error mapper MUST preserve the current project's diagnostic logging for compression edge cases.

## Affected Features

- F-001 (Cross-Request Cache): conversation_id reuse creates correlation risk with anti-detection.
- F-003 (Error Mapping): Must preserve compression-aware error diagnostics.
- F-004 (Converter Enhancement): Tool name shortening changes the request shape, affecting both compression efficiency and anti-detection fingerprint consistency.
- F-008 (CredentialIdentity): The trait design directly mediates the boundary between anti-detection and caching concerns.

## Recommendation

All P0/P1 feature implementations MUST include an "anti-detection impact assessment" section in their design documents. The assessment should answer: (1) Does this feature create new temporal or structural correlation patterns? (2) Does it change request shape in ways that affect compression efficiency? (3) Does it alter the fingerprint surface visible to the upstream API? If any answer is yes, the feature design MUST include specific mitigations.
