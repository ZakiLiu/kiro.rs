# F-005 — System Prompt Presets

> Role: test-strategist | Related decisions: TS-07

## Architecture

Prompt presets is a P2 feature with lower risk. The test suite SHOULD focus on preset loading, validation, and injection. Dev-source implements five built-in presets (override, pentest, nsfw, code_complete, concise) per design-research. The test strategy for this feature is test-after rather than test-first.

Key testable components:
- **Preset loading** — Parse preset definitions from configuration (JSON or YAML).
- **Preset validation** — Reject malformed, empty, or excessively large presets.
- **System prompt injection** — Prepend/append preset content to the request's system message.
- **Admin UI integration** — Preset selection via API (testable at handler level, not UI level).

## Interface Contract

- `PresetLibrary::load(config: &PresetConfig) -> Result<Self>` — MUST return error for invalid config, not panic.
- `PresetLibrary::get(name: &str) -> Option<&Preset>` — Lookup by name.
- `PresetLibrary::apply(preset: &Preset, request: &mut MessagesRequest)` — Inject system prompt.

## Constraints (RFC 2119)

- Preset loading SHOULD gracefully handle malformed configuration with descriptive errors.
- Preset injection MUST NOT modify user messages — only system prompt is affected.
- Empty preset name SHOULD return None from get, not panic.
- Preset content SHOULD be size-bounded (e.g., ≤ 10KB) to prevent accidental context window exhaustion.
- The prompt filter feature (dev-source: restriction stripping) MAY be tested separately if introduced.

## Test Approach

**Unit tests (≥ 8 tests):**
1. Load valid preset config — success.
2. Load malformed JSON — error returned.
3. Load empty config — empty library, no error.
4. Get existing preset by name.
5. Get non-existent preset — returns None.
6. Apply preset — system message updated.
7. Apply preset to request without system message — system message created.
8. Preset size validation — oversized preset rejected.

**No property-based tests** — Low complexity, deterministic inputs.

**No integration tests** — Preset is self-contained; interaction with other modules is minimal.

## TODOs

- Decide whether presets are loaded from embedded defaults, config file, or both.
- Determine if filter functionality (restriction stripping) will be included; if so, add regex-based tests.
- Define preset schema for validation tests.
