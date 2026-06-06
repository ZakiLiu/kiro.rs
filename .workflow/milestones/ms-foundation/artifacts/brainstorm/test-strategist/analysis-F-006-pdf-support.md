# F-006 — PDF Document Support

> Role: test-strategist | Related decisions: TS-08

## Architecture

PDF extraction is a P2 feature that introduces an external dependency (`lopdf` crate per dev-source). The test suite MUST use real PDF fixture files to validate extraction correctness across document variations. The existing image.rs (5 tests) provides a pattern for media processing test structure.

Key testable components:
- **Base64 decoding** — Document block contains base64-encoded PDF; decoding MUST handle padding variations.
- **Text extraction** — `lopdf` page iteration and text extraction.
- **Error handling** — Malformed PDF, encrypted PDF, empty PDF, zero-page PDF.
- **CJK text** — Chinese/Japanese/Korean characters MUST be extracted correctly.
- **Size limits** — Very large PDFs SHOULD be bounded to prevent memory exhaustion.

## Interface Contract

- `extract_pdf_text(base64_data: &str) -> Result<String>` — Pure function, returns extracted text or error.
- `is_pdf(media_type: &str) -> bool` — Content type detection.
- Integration with converter: PDF document blocks in Anthropic requests MUST be converted to text content blocks for Kiro.

## Constraints (RFC 2119)

- PDF extraction MUST handle malformed PDF input without panic (return error).
- Encrypted PDFs MUST return a descriptive error, not crash.
- Empty PDFs (zero pages) MUST return empty string, not error.
- Base64 decoding MUST handle standard and URL-safe variants.
- Extracted text SHOULD preserve paragraph structure (newline-separated).
- PDF processing MUST have a size limit (MAY be configurable) to prevent OOM on malicious input.

## Test Approach

**Unit tests (≥ 8 tests):**
1. Valid simple PDF — text extracted correctly.
2. Multi-page PDF — all pages extracted in order.
3. CJK content PDF — characters preserved.
4. Empty PDF (zero pages) — returns empty string.
5. Malformed base64 — error returned.
6. Malformed PDF binary — error returned, no panic.
7. Encrypted PDF — descriptive error returned.
8. Large PDF — size limit enforced.

**Test fixtures:**
The `tests/fixtures/pdf/` directory MUST contain:
- `simple.pdf` — Single-page, ASCII text.
- `multipage.pdf` — Three pages with distinct content.
- `cjk.pdf` — Chinese text document.
- `empty.pdf` — Valid PDF with zero content pages.
- `encrypted.pdf` — Password-protected PDF.

**No property-based tests** — PDF parsing is format-dependent, not amenable to random generation.

## TODOs

- Create or source test PDF fixtures (consider generating with a PDF library in a build script).
- Evaluate `lopdf` vs `pdf-extract` crate for text extraction quality.
- Determine size limit threshold (e.g., 10MB raw, 50 pages).
- Verify `lopdf` compatibility with Rust 2024 edition.
