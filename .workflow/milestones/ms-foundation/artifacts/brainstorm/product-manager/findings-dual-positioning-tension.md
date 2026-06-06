# Finding: Dual Positioning Tension Between "Swiss Army Knife" and "Production Platform"

> Role: product-manager | Impact: HIGH

## Description

The user context (PM-03) explicitly requires balancing two product archetypes: the current "Swiss Army Knife" (scale + evasion + compression, 30k LOC) and dev-source's "Production Platform" (admin control + observability, 13k LOC). These archetypes serve overlapping but distinct user segments with different value drivers.

The Swiss Army Knife user prioritizes resilience, stealth, and handling edge cases (large payloads, GIF extraction, fingerprint diversity). The Production Platform user prioritizes operational visibility, administrative control, and API compliance. Attempting to serve both without a clear hierarchy creates three risks:

1. **Feature sprawl**: The combined feature set (11 unique from current + 13 from dev-source) would approximately double the maintenance surface.
2. **Configuration complexity**: Users who need stealth features do not necessarily need prompt presets, and vice versa. Without feature gating, all users bear the cognitive load of all features.
3. **Performance budget**: Anti-detection overhead (fingerprint generation, affinity binding, rate limiting) and metrics overhead compete for the same per-request latency budget.

## Affected Features

All features are affected, but the tension is most acute in:
- F-001 (cache) vs anti-detection: cache benefits from stable identifiers while anti-detection benefits from identity diversity
- F-005 (prompt presets) vs compression pipeline: both modify the request payload in the converter pipeline, creating ordering dependencies
- F-002 (metrics) vs stealth: detailed metrics could inadvertently expose patterns that anti-detection aims to obscure

## Recommendation

Adopt a "layered defaults" strategy: the core proxy (protocol conversion, credential failover, error mapping) is always active. Stealth features (anti-detection, fingerprint, rate limiting) and platform features (metrics, presets, PDF) are independently toggleable feature groups via config. This allows a single binary to serve both archetypes without forcing users into configuration they do not need.
