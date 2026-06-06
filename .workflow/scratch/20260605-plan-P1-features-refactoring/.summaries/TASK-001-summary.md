# TASK-001: Prompt Presets — config model + header injection + Admin CRUD API

## Changes
- `src/model/config.rs`: Added `Preset` struct (id, name, system_prompt, enabled) with serde camelCase. Added `presets: Vec<Preset>` field to `Config` with `#[serde(default)]`. Added 3 unit tests for deserialization.
- `src/anthropic/middleware.rs`: Added `presets: Arc<RwLock<Vec<Preset>>>` field to `AppState` with `with_presets()` builder method.
- `src/anthropic/router.rs`: Added `presets` parameter to `create_router_with_provider()`, wires into AppState. Added `#[allow(clippy::too_many_arguments)]` (8th param).
- `src/anthropic/handlers.rs`: Added `HeaderMap` extractor to `post_messages`. Extracts `x-preset-id` header, looks up preset by ID (must be enabled), prepends `system_prompt` as first SystemMessage before `convert_request`.
- `src/admin/types.rs`: Added `CreatePresetRequest` and `UpdatePresetRequest` structs with serde camelCase.
- `src/admin/middleware.rs`: Added `presets: Arc<RwLock<Vec<Preset>>>` field to `AdminState` with `with_presets()` builder.
- `src/admin/handlers.rs`: Added `get_presets`, `create_preset`, `update_preset`, `delete_preset` handlers. Create returns 201, Delete returns 204, duplicate ID returns 409 Conflict.
- `src/admin/router.rs`: Registered `GET/POST /presets` and `PUT/DELETE /presets/{id}` routes.
- `src/main.rs`: Constructs `Arc<RwLock<Vec<Preset>>>` from config, passes to both `create_router_with_provider` and `AdminState`.
- `src/anthropic/converter.rs`: Fixed pre-existing clippy collapsible_if warnings (let-chain style).

## Verification
- [x] Config with `presets` array deserializes correctly: verified by `test_config_deserializes_presets_array` test
- [x] Request with `x-preset-id: test` header has preset.system_prompt prepended: verified by code inspection — preset_msg inserted at index 0 of `payload.system`
- [x] Request without `x-preset-id` header or with unknown preset id proceeds normally: verified — header extraction is guarded by `if let Some(...)`, unknown ID logs debug and continues
- [x] Admin GET /admin/presets returns JSON array: verified — handler reads `state.presets.read().clone()` and returns `Json(presets)`
- [x] Admin POST /admin/presets creates a new preset and returns 201: verified — handler returns `StatusCode::CREATED` with created preset
- [x] Admin DELETE /admin/presets/{id} removes the preset and returns 204: verified — handler returns `StatusCode::NO_CONTENT`
- [x] `cargo test` all green, `cargo clippy` no warnings: 436 tests passed, 0 clippy warnings

## Tests
- [x] `cargo test`: 436 passed, 0 failed
- [x] `cargo clippy -- -D warnings`: 0 warnings (also fixed 5 pre-existing clippy issues)

## Deviations
- Fixed 5 pre-existing clippy warnings (collapsible_if x2, doc_lazy_continuation, collapsible_match, unnecessary_cast) — these are outside strict task scope but were required to meet the convergence criteria of `cargo clippy -- -D warnings` passing clean.
- Added `#[allow(clippy::too_many_arguments)]` to `create_router_with_provider` since adding the presets parameter pushed arg count to 8 (limit 7). Refactoring the function signature to use a builder/config struct would be a larger change outside scope.
- `Cargo.toml` and `Cargo.lock` changes were present in the working tree (likely from another change) and were NOT included in this commit.

## Notes
- Presets are stored as `Arc<RwLock<Vec<Preset>>>` shared between `AppState` (read path for request handling) and `AdminState` (write path for Admin CRUD). This ensures Admin API mutations are immediately visible to the request handler.
- Runtime preset changes via Admin API are ephemeral (lost on restart). Config persistence is out of scope for P1.
- Preset injection happens BEFORE `convert_request` and BEFORE compression, as specified.
- PUT /presets/{id} supports partial updates (all fields optional).
