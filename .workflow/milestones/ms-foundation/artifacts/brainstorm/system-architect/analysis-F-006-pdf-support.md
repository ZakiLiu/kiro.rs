# F-006 -- PDF Document Support

> Role: system-architect | Related decisions: SA-10, PM-01

## Architecture

A new module `src/anthropic/document.rs` handles PDF content blocks in Anthropic API requests. When a message contains a document block with `type: "document"` and `source.type: "base64"` with `media_type: "application/pdf"`, the document handler extracts text from the PDF and converts it to a text content block before Kiro conversion.

**Module placement:** `src/anthropic/document.rs` within the anthropic module, invoked during converter processing. This mirrors the dev-source document.rs placement.

**Dependency:** Adds `lopdf = "0.32"` to Cargo.toml. The lopdf crate provides pure-Rust PDF parsing without external C dependencies, consistent with the current build approach. Alternative: `pdf-extract` crate which wraps lopdf with higher-level text extraction.

**Integration flow:**
1. converter::convert_request() iterates content blocks
2. For document blocks with PDF media type, base64-decode the source data
3. Pass decoded bytes to PdfExtractor::extract_text()
4. Replace the document block with a text block containing extracted text
5. Prefix extracted text with metadata (page count, source filename if available)
6. Continue with standard conversion pipeline (compression may apply to extracted text)

**Size considerations:** PDF text extraction can produce large output. The extraction MUST cap at 100 pages and SHOULD truncate individual page output to prevent excessive request size. The existing compression pipeline (SA-06) provides a safety net for oversized requests.

## Interface Contract

```rust
pub struct PdfExtractor;

impl PdfExtractor {
    pub fn extract_text(data: &[u8], max_pages: usize) -> Result<PdfContent, PdfError>;
}

pub struct PdfContent {
    pub text: String,
    pub page_count: usize,
    pub truncated: bool,
}

pub enum PdfError {
    InvalidPdf(String),
    TooLarge { pages: usize, limit: usize },
    ExtractionFailed(String),
}
```

## Constraints (RFC 2119)

- SHOULD add lopdf dependency for PDF parsing
- MUST handle malformed PDFs gracefully -- return a descriptive error message in the text block rather than panicking
- MUST cap extraction at configurable max_pages (default 100) to prevent memory exhaustion
- MUST base64-decode PDF data before extraction
- SHOULD preserve document ordering (pages extracted in order)
- MUST NOT block the event loop -- PDF extraction SHOULD run in spawn_blocking if it exceeds a time threshold
- MAY integrate with compression pipeline for extracted text that exceeds size limits
- MUST be feature-gated (config.pdf_support_enabled) to allow disabling without recompilation

## Test Approach

- **Unit tests:** Text extraction from known PDF files (small, multi-page, scanned/empty). Error handling for corrupted PDFs. Page limit enforcement. Base64 decode + extraction pipeline.
- **Integration tests:** Full request with PDF document block produces text response. PDF block in multi-content message preserved alongside other blocks.
- **Edge cases:** Zero-page PDF. Password-protected PDF (should fail gracefully). PDF with only images (no text -- return empty or descriptive message). Extremely large PDF (100+ pages, verify truncation).
- **Performance:** Measure extraction time for 50-page PDF to determine if spawn_blocking is needed.

## TODOs

- Evaluate lopdf vs pdf-extract crate (text extraction quality, dependency size, maintenance status)
- Determine if scanned PDFs (image-only) should return a warning message or empty text
- Profile memory usage during PDF extraction for large documents
- Consider adding a per-document size limit (bytes) in addition to page limit
- Assess whether PDF extraction results should be cached (unlikely needed given ephemeral nature)
