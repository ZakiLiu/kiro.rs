# F-006 — PDF Document Support

> Role: product-manager | Related decisions: PM-01, PM-04

## Architecture

PDF support broadens the multimodal capabilities of the proxy. The current project excels at image processing (GIF extraction, format conversion) but lacks document handling. Dev-source implements PDF text extraction via the lopdf crate, handling base64-encoded PDF content blocks in Anthropic API requests.

Product value: Claude API clients increasingly send PDF documents (research papers, contracts, documentation). Without PDF support, these requests either fail or require client-side preprocessing. Adding PDF handling completes the document pipeline alongside existing image processing.

This is a P2 feature — lower urgency than cache/metrics/error-mapping, but straightforward to implement with bounded scope. The lopdf dependency is lightweight and well-maintained.

Target module: `src/anthropic/document.rs` handling PDF content blocks in the converter pipeline.

## Interface Contract

- **Input**: Anthropic message content block with type "document" and base64-encoded PDF data
- **Output**: Extracted text content injected as text blocks, preserving document structure where possible
- **Fallback**: If PDF extraction fails (encrypted, corrupted), MUST pass the original content block through and log a warning
- **Size limits**: MUST respect existing request size constraints; extracted text contributes to input token count

## Constraints (RFC 2119)

- MUST extract readable text from standard PDF documents
- MUST handle base64 decoding of PDF content blocks
- MUST fail gracefully on encrypted or malformed PDFs (log warning, pass through or return informative error)
- MUST NOT block the request pipeline on slow PDF extraction (timeout after configurable limit, default 5 seconds)
- SHOULD preserve basic document structure (paragraphs, headings) in extracted text
- MAY support PDF metadata extraction (title, author) as supplementary context

## Test Approach

- Unit tests for PDF text extraction from sample documents (simple text, multi-page, tables)
- Unit tests for base64 decode to PDF parse pipeline
- Negative tests: encrypted PDF, corrupted PDF, zero-length PDF
- Integration test: send Anthropic request with PDF content block, verify text extraction in converted Kiro request
- Performance test: verify extraction completes within timeout for documents up to 50 pages

## TODOs

- Add lopdf dependency to Cargo.toml (evaluate version compatibility with existing dependency tree)
- Create sample PDF test fixtures (simple, complex, encrypted)
- Study dev-source document.rs for extraction approach and edge case handling
- Determine if PDF extraction should be configurable (enable/disable via config)
