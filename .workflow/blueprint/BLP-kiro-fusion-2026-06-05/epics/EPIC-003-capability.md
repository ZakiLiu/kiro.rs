---
document: epic
session_id: BLP-kiro-fusion-2026-06-05
epic_id: EPIC-003
title: P2 Capability Breadth
priority: P2
mvp: false
features: [F-005, F-006]
constraints: [C-004]
---

# EPIC-003: P2 Capability Breadth

The capability epic adds two user-facing features — prompt presets and PDF support — that expand the proxy's functionality beyond pass-through. These are additive features with no impact on existing code paths when unused, making them low-risk additions after the foundation and reliability layers are stable.

Both features follow constraint C-004: reference dev-source designs but reimplement independently to match kiro.rs module style.

## Stories Summary

| ID | Title | Size | Trace | Dependencies |
|----|-------|------|-------|-------------|
| S-010 | Prompt preset loading and storage | M | F-005 | EPIC-001 (AppState extension) |
| S-011 | Admin API preset management UI | M | F-005 | S-010 |
| S-012 | PDF text extraction with lopdf | M | F-006 | None |

## Story Details

### S-010: Implement Prompt Preset Loading and Storage

**User Story**: As a power user, I want configurable system prompt presets that are automatically injected into requests based on preset selection, so that I can maintain consistent system prompts across sessions without client-side configuration.

**Size**: M (3 pts)

**Trace**: F-005, C-004

**Acceptance Criteria**:
1. A `PresetStore` MUST load presets from a JSON/TOML file at startup and support runtime reload via Admin API
2. Each preset MUST contain: id, name, system_prompt (string), tags (optional), created_at
3. Preset selection MUST be via a custom header (`x-preset-id`) or query parameter; when absent, no preset is applied
4. When a preset is active, the system prompt MUST be prepended to (not replace) any existing system message in the request
5. Preset injection MUST occur before compression (so compressed requests include preset content)

**Dependencies**: EPIC-001 complete (AppState extension pattern established)

**Open Question**: OQ-2 — prompt filter security implications must be assessed before this story is approved for development.

---

### S-011: Extend Admin API for Preset Management UI

**User Story**: As a platform operator, I want Admin API endpoints and a basic UI panel to create, update, delete, and preview prompt presets, so that I can manage presets without editing config files.

**Size**: M (3 pts)

**Trace**: F-005

**Acceptance Criteria**:
1. `POST /admin/presets` MUST create a new preset with validation (name required, system_prompt non-empty)
2. `PUT /admin/presets/:id` MUST update an existing preset; `DELETE /admin/presets/:id` MUST remove it
3. `GET /admin/presets` MUST list all presets with pagination support
4. Admin UI MUST include a preset management panel with create/edit/delete actions and a preview of the system prompt text

**Dependencies**: S-010 (PresetStore must exist)

---

### S-012: Implement PDF Text Extraction with lopdf

**User Story**: As a power user, I want PDF documents sent as base64-encoded `document` content blocks to be automatically extracted to text before forwarding to upstream, so that I can include PDF content in conversations without manual text extraction.

**Size**: M (3 pts)

**Trace**: F-006, C-004

**Acceptance Criteria**:
1. Converter MUST detect `document` content blocks with `media_type: "application/pdf"` and extract text using the `lopdf` crate
2. Extracted text MUST replace the PDF document block with a `text` content block, preserving the block's position in the message
3. Extraction failure (encrypted PDF, image-only PDF) MUST NOT cause request failure; instead, a fallback text block MUST be inserted: `[PDF extraction failed: {reason}]`
4. PDF processing MUST respect a size limit (configurable, default 10MB raw) to prevent memory exhaustion

**Dependencies**: None — this is converter-internal and can be developed independently.

---

## Epic-Level Acceptance Criteria

1. All 3 stories completed and merged
2. Preset injection works end-to-end: config file → header selection → system prompt prepend → upstream request
3. PDF extraction handles common PDFs (text-based, multi-page) correctly
4. Edge cases handled gracefully: encrypted PDF, empty preset, missing header
5. No regressions in existing functionality (`cargo test` green)

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| OQ-2 unresolved blocks S-010 | Medium | High | Gate S-010 on OQ-2 resolution; S-012 can proceed independently |
| lopdf crate quality/maintenance | Low | Medium | Evaluate `pdf-extract` as alternative; wrap in trait for easy swap |
| Preset injection interacts badly with compression | Low | Medium | Test preset + compression pipeline end-to-end; preset content counts toward compression budget |
| Large PDF causes OOM | Low | High | S-012 enforces size limit + streaming extraction if lopdf supports it |
