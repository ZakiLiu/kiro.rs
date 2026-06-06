# Finding: Prompt Filter Carries Non-Trivial Legal and Reputational Risk

> Role: product-manager | Impact: MEDIUM

## Description

Dev-source's prompt_filter.rs implements system prompt restriction stripping with 14+ regex patterns designed to remove safety guardrails from model responses. While PM-05 correctly separates prompt presets (SHOULD adopt) from prompt filter (needs evaluation), the evaluation criteria deserve explicit documentation.

The prompt filter feature creates three categories of risk:
1. **Terms of Service**: Stripping safety instructions injected by the upstream provider likely violates API terms of service. If detected, this could result in credential revocation or account bans.
2. **Legal liability**: Depending on jurisdiction, actively circumventing AI safety measures may expose operators to liability for harmful outputs.
3. **Detection surface**: The filter modifies system prompts in detectable ways. If the upstream provider audits system prompt patterns, filtered prompts create a recognizable signature that undermines anti-detection (contradicting PM-02).

## Affected Features

- F-005 (prompt presets): The presets feature MUST be cleanly separable from the filter feature. Shipping them together would force users to accept filter risk to get preset value.
- Anti-detection system: Prompt filtering creates a detectable behavioral signature that conflicts with anti-detection goals.

## Recommendation

The prompt filter MUST NOT be included in the initial release. If evaluated for future inclusion, it SHOULD be behind a compile-time feature flag (not just runtime config) so that binary distribution does not include the capability by default. Prompt presets SHOULD proceed independently on the P2 timeline.
