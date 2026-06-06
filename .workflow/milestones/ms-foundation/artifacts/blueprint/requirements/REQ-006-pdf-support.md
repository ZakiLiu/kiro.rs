---
document: requirement
session_id: BLP-kiro-fusion-2026-06-05
req_id: REQ-006
priority: should
wave: P2
---

# REQ-006: PDF Document Support

## User Story

As a **Power User**, I want to include PDF documents in my API requests via base64-encoded content blocks, so that the proxy extracts the text content and forwards it to Kiro in a format the model can process, completing the multimodal content pipeline alongside existing image support.

## Description

The current kiro.rs handles image content blocks (including GIF frame extraction via `image.rs`) but has no support for PDF documents. When a client sends a `document` content block with `type: "base64"` and `media_type: "application/pdf"`, the proxy currently passes it through unchanged — which Kiro may not support natively.

PDF support adds text extraction from base64-encoded PDF content blocks during the request conversion phase. The extraction uses the `lopdf` crate (or equivalent) to parse the PDF and extract text content, which is then converted into a `text` content block that replaces the original `document` block. This follows the same integration pattern as `image.rs`: content block processing during conversion, with graceful fallback on failure.

The `lopdf` dependency MUST be feature-gated (`pdf-support` feature flag) to avoid increasing binary size for users who don't need PDF processing. When the feature is disabled at compile time, PDF document blocks are passed through unchanged (no extraction attempt).

## Acceptance Criteria

1. **Text Extraction**: When a request contains a `document` content block with `media_type: "application/pdf"` and base64-encoded data, the system MUST decode the base64, parse the PDF, and extract text content. The extracted text MUST replace the original document block as a `text` content block.

2. **Size Limits**: The system MUST enforce a maximum PDF file size of 32 MB (after base64 decoding). The extracted text MUST be capped at 200,000 characters. PDFs exceeding the size limit MUST be rejected with a clear error message. Text exceeding the character limit MUST be truncated with a `[truncated]` marker.

3. **Graceful Failure**: Malformed, encrypted, or unparseable PDFs MUST NOT cause a panic or crash. On extraction failure, the system MUST log a warning and either: (a) pass the original document block through unchanged, or (b) replace it with a text block containing an extraction failure notice. The choice SHOULD be configurable.

4. **Feature Gate**: The `lopdf` dependency and all PDF processing code MUST be behind a `pdf-support` Cargo feature flag. When the feature is disabled, PDF document blocks MUST be passed through unchanged with no processing attempt.

## Configuration

```json
{
  "pdfSupportEnabled": true
}
```

Runtime configuration controls whether PDF extraction is attempted even when the feature is compiled in. This allows disabling PDF processing without recompilation.

## Processing Pipeline

```
Content Block (document, application/pdf, base64)
  → base64 decode (reject if > 32MB)
  → lopdf parse (fallback on failure)
  → text extraction (truncate at 200K chars)
  → Replace with text content block
```

## Dependencies

| REQ | Relationship |
|-----|-------------|
| REQ-004 | **Soft** — PDF processing occurs in the converter alongside other content block transforms |
| REQ-007 | **Soft** — converter.rs split may create a dedicated content block processing sub-module |
| REQ-003 | **Soft** — Extraction failures may produce errors that flow through ErrorMapper |

## Brainstorm Trace

| Decision | Role | Relevance |
|----------|------|-----------|
| SA-10 | System Architect | lopdf integration into content block processing |
| PM-10 | Product Manager | Completes multimodal pipeline alongside image processing |
| SME-09 | Subject Matter Expert | Feature-gated dependency, graceful failure required |
| TS-08 | Test Strategist | Real PDF fixtures: empty, encrypted, CJK, large |

## Parallels with Image Processing

| Aspect | Image (`image.rs`) | PDF (this REQ) |
|--------|-------------------|----------------|
| Input | base64 image data | base64 PDF data |
| Processing | Resize, GIF frame extraction | Text extraction |
| Output | Processed image content blocks | Text content block |
| Size limit | 4Mpx / 4000px long edge | 32MB file / 200K chars |
| Failure mode | Pass through on error | Pass through or notice on error |
| Feature gate | Always compiled | `pdf-support` feature flag |

## Security Considerations

- `lopdf` crate security posture and fuzzing coverage SHOULD be evaluated before adoption (see SA outstanding TODOs).
- PDF parsing is a known attack surface. The extraction MUST run with no filesystem access beyond the in-memory buffer.
- Encrypted PDFs MUST NOT be decrypted — they SHOULD be treated as extraction failures.
- Page count limit: extraction SHOULD cap at 100 pages to prevent CPU exhaustion on adversarial inputs.
