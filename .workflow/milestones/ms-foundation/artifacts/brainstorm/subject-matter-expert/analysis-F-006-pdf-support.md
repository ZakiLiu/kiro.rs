# F-006 — PDF Document Support

> Role: subject-matter-expert | Related decisions: PM-01

## Architecture

The dev-source implements PDF handling via `lopdf = "0.32"` in `document.rs`: base64-encoded PDF content blocks are decoded, text is extracted page-by-page, and the result is injected as plain text content blocks into the Kiro request.

The current project already handles image content blocks (`image.rs` with GIF extraction, format conversion). PDF support follows the same pattern — intercept document-type content blocks during protocol conversion, extract text, and substitute.

Proposed module: `anthropic/document.rs` — handles `"type": "document"` content blocks with `"media_type": "application/pdf"`.

Processing pipeline:
1. Detect `document` blocks in `convert_request()`
2. Base64-decode the `data` field
3. Extract text via `lopdf` page iterator
4. Replace the document block with a text block containing extracted content
5. If extraction fails (encrypted PDF, image-only PDF), log a warning and pass through a placeholder message

## Interface Contract

```rust
pub fn extract_pdf_text(base64_data: &str) -> Result<String, PdfError>;

pub enum PdfError {
    Base64Decode(String),
    ParseFailed(String),
    Encrypted,
    NoTextContent,
}
```

Consumers: `anthropic/converter.rs` (content block processing).

## Constraints (RFC 2119)

- PDF extraction MUST handle common edge cases: encrypted PDFs (return error, do not crash), empty pages, non-Latin scripts.
- PDF text extraction SHOULD preserve paragraph structure with double-newline separators between pages.
- The `lopdf` dependency MUST be feature-gated (`pdf` feature flag) to avoid increasing binary size for users who do not need PDF support.
- PDF content MUST be counted toward token estimation for compression decisions.
- Extraction failures MUST NOT cause request failures — fall back to a text block stating `[PDF content could not be extracted]`.

## Test Approach

- Unit tests: Extract text from a known PDF fixture, verify against expected output.
- Edge cases: Encrypted PDF, image-only PDF, zero-page PDF, PDF with CJK text.
- Performance: Measure extraction time for a 50-page PDF to ensure it does not block the async runtime.

## TODOs

- Evaluate `lopdf` vs `pdf-extract` crate for text extraction quality, especially for complex layouts.
- Create test fixture PDFs covering the edge cases above.
- Determine maximum PDF size to accept (5MB upstream limit applies to the full request body, so PDF text expansion matters).
