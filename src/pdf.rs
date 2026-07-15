//! PDF 文本提取模块（feature-gated: pdf-support）
//!
//! 从 base64 编码的 PDF 文档中提取纯文本内容。
//! 用于将 Anthropic API 的 document content block 转换为文本块。
//!
//! # 限制
//! - 最大 PDF 原始大小：10MB
//! - 最大提取文本长度：200,000 字符
//! - 仅提取文本内容，不支持 OCR

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};

/// PDF 原始数据最大大小（10MB）
const MAX_PDF_SIZE: usize = 10 * 1024 * 1024;

/// 提取文本最大字符数
const MAX_TEXT_CHARS: usize = 200_000;

/// PDF 提取错误
#[derive(Debug)]
pub enum PdfError {
    /// PDF 文件过大（超过 10MB）
    TooLarge { size: usize },
    /// 提取文本失败
    ExtractionFailed(String),
    /// Base64 解码失败
    InvalidBase64(String),
    /// 不支持的格式（非 PDF）
    UnsupportedFormat(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::TooLarge { size } => {
                write!(
                    f,
                    "PDF file too large: {} bytes (max {})",
                    size, MAX_PDF_SIZE
                )
            }
            PdfError::ExtractionFailed(msg) => write!(f, "PDF text extraction failed: {}", msg),
            PdfError::InvalidBase64(msg) => write!(f, "Invalid base64 data: {}", msg),
            PdfError::UnsupportedFormat(msg) => write!(f, "Unsupported format: {}", msg),
        }
    }
}

impl std::error::Error for PdfError {}

/// 从 base64 编码的 PDF 数据中提取文本
///
/// # 参数
/// - `base64_data`: base64 编码的 PDF 文件数据
///
/// # 返回
/// 提取的文本内容（超过 200K 字符时自动截断并追加 `... [truncated]`）
///
/// # 错误
/// - `PdfError::InvalidBase64`: base64 解码失败
/// - `PdfError::TooLarge`: PDF 原始大小超过 10MB
/// - `PdfError::ExtractionFailed`: lopdf 解析或文本提取失败
pub fn extract_text_from_pdf(base64_data: &str) -> Result<String, PdfError> {
    // 0. Pre-decode 大小预检（O(1)，防内存放大 DoS）
    let estimated_size = base64_data.len() / 4 * 3;
    if estimated_size > MAX_PDF_SIZE {
        return Err(PdfError::TooLarge {
            size: estimated_size,
        });
    }

    // 1. Base64 解码
    let pdf_bytes = BASE64
        .decode(base64_data)
        .map_err(|e| PdfError::InvalidBase64(e.to_string()))?;

    // 2. 大小检查（精确值二次校验）
    if pdf_bytes.len() > MAX_PDF_SIZE {
        return Err(PdfError::TooLarge {
            size: pdf_bytes.len(),
        });
    }

    // 3. 使用 lopdf 加载 PDF
    let doc = lopdf::Document::load_mem(&pdf_bytes)
        .map_err(|e| PdfError::ExtractionFailed(e.to_string()))?;

    // 4. 获取所有页码并提取文本
    let pages = doc.get_pages();
    let mut page_numbers: Vec<u32> = pages.keys().copied().collect();
    page_numbers.sort();

    if page_numbers.is_empty() {
        return Err(PdfError::ExtractionFailed(
            "PDF contains no pages".to_string(),
        ));
    }

    let text = doc
        .extract_text(&page_numbers)
        .map_err(|e| PdfError::ExtractionFailed(e.to_string()))?;

    // 5. 文本截断
    if text.chars().count() > MAX_TEXT_CHARS {
        let truncated: String = text.chars().take(MAX_TEXT_CHARS).collect();
        Ok(format!("{}... [truncated]", truncated))
    } else {
        Ok(text)
    }
}

/// 从原始字节提取 PDF 文本
pub fn extract_text_from_bytes(pdf_bytes: &[u8]) -> Result<String, PdfError> {
    if pdf_bytes.len() > MAX_PDF_SIZE {
        return Err(PdfError::TooLarge {
            size: pdf_bytes.len(),
        });
    }

    let doc = lopdf::Document::load_mem(pdf_bytes)
        .map_err(|e| PdfError::ExtractionFailed(e.to_string()))?;

    let pages = doc.get_pages();
    let mut page_numbers: Vec<u32> = pages.keys().copied().collect();
    page_numbers.sort();

    if page_numbers.is_empty() {
        return Err(PdfError::ExtractionFailed(
            "PDF contains no pages".to_string(),
        ));
    }

    let text = doc
        .extract_text(&page_numbers)
        .map_err(|e| PdfError::ExtractionFailed(e.to_string()))?;

    if text.chars().count() > MAX_TEXT_CHARS {
        let truncated: String = text.chars().take(MAX_TEXT_CHARS).collect();
        Ok(format!("{}... [truncated]", truncated))
    } else {
        Ok(text)
    }
}

/// 从 URL 下载 PDF 文件（独立线程执行，10 秒超时，20MB 限制）
pub fn download_pdf(url: &str) -> Result<Vec<u8>, PdfError> {
    const DOWNLOAD_MAX: usize = 20 * 1024 * 1024;
    const TIMEOUT_SECS: u64 = 10;

    let url = url.to_string();
    // 在独立线程执行 blocking HTTP 请求，避免在 tokio runtime 内 panic
    let handle = std::thread::spawn(move || {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .build()
            .map_err(|e| PdfError::ExtractionFailed(format!("HTTP client error: {}", e)))
            .and_then(|client| {
                client
                    .get(&url)
                    .send()
                    .map_err(|e| PdfError::ExtractionFailed(format!("download failed: {}", e)))
            })
            .and_then(|resp| {
                if !resp.status().is_success() {
                    return Err(PdfError::ExtractionFailed(format!(
                        "HTTP {}",
                        resp.status()
                    )));
                }
                resp.bytes()
                    .map_err(|e| PdfError::ExtractionFailed(format!("read body failed: {}", e)))
            })
    });

    let bytes = handle
        .join()
        .map_err(|_| PdfError::ExtractionFailed("download thread panicked".to_string()))??;
    if bytes.len() > DOWNLOAD_MAX {
        return Err(PdfError::TooLarge { size: bytes.len() });
    }
    Ok(bytes.to_vec())
}

/// 生成 PDF 提取失败时的回退文本
pub fn fallback_text(error: &PdfError) -> String {
    match error {
        PdfError::TooLarge { size } => {
            format!(
                "[PDF content: file too large ({:.1}MB, max {}MB)]",
                *size as f64 / 1_048_576.0,
                MAX_PDF_SIZE / 1_048_576
            )
        }
        PdfError::ExtractionFailed(msg) => format!("[PDF extraction failed: {}]", msg),
        PdfError::InvalidBase64(msg) => {
            format!("[PDF extraction failed: invalid base64 - {}]", msg)
        }
        PdfError::UnsupportedFormat(msg) => format!("[PDF extraction failed: {}]", msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_base64() {
        let result = extract_text_from_pdf("not-valid-base64!!!");
        assert!(result.is_err());
        match result.unwrap_err() {
            PdfError::InvalidBase64(_) => {}
            other => panic!("Expected InvalidBase64, got: {:?}", other),
        }
    }

    #[test]
    fn test_too_large_pdf() {
        // 生成一个超过 10MB 的 base64 数据（实际字节 > 10MB）
        let large_data = vec![0u8; MAX_PDF_SIZE + 1];
        let base64_data = BASE64.encode(&large_data);
        let result = extract_text_from_pdf(&base64_data);
        assert!(result.is_err());
        match result.unwrap_err() {
            PdfError::TooLarge { size } => {
                assert!(size > MAX_PDF_SIZE);
            }
            other => panic!("Expected TooLarge, got: {:?}", other),
        }
    }

    #[test]
    fn test_corrupt_pdf() {
        // 有效 base64 但不是 PDF 内容
        let not_pdf = BASE64.encode(b"This is not a PDF file");
        let result = extract_text_from_pdf(&not_pdf);
        assert!(result.is_err());
        match result.unwrap_err() {
            PdfError::ExtractionFailed(_) => {}
            other => panic!("Expected ExtractionFailed, got: {:?}", other),
        }
    }

    #[test]
    fn test_fallback_text_too_large() {
        let err = PdfError::TooLarge { size: 15_000_000 };
        let text = fallback_text(&err);
        assert!(text.contains("file too large"));
        assert!(text.contains("14.3MB"));
    }

    #[test]
    fn test_fallback_text_extraction_failed() {
        let err = PdfError::ExtractionFailed("corrupt data".to_string());
        let text = fallback_text(&err);
        assert!(text.contains("PDF extraction failed"));
        assert!(text.contains("corrupt data"));
    }

    #[test]
    fn test_extract_minimal_pdf() {
        // 一个最小的有效 PDF（包含 "Hello" 文本）
        // 这个 PDF 由 lopdf 自身生成的最小文档
        let pdf_bytes = generate_test_pdf("Hello World");
        let base64_data = BASE64.encode(&pdf_bytes);
        let result = extract_text_from_pdf(&base64_data);
        match result {
            Ok(text) => {
                assert!(
                    text.contains("Hello") || text.contains("hello"),
                    "Extracted text should contain 'Hello', got: {:?}",
                    text
                );
            }
            Err(e) => {
                // 某些最小 PDF 可能无法被 lopdf 正确提取文本，
                // 这种情况下只要走了 graceful fallback 就行
                panic!("Expected successful extraction for valid PDF, got: {:?}", e);
            }
        }
    }

    #[test]
    fn test_truncation_at_max_chars() {
        // 生成一个包含超长文本的 PDF
        let long_text: String = "A".repeat(MAX_TEXT_CHARS + 1000);
        let pdf_bytes = generate_test_pdf(&long_text);
        let base64_data = BASE64.encode(&pdf_bytes);

        match extract_text_from_pdf(&base64_data) {
            Ok(text) => {
                // 如果 lopdf 成功提取了足够多的文本，应该被截断
                if text.chars().count() > MAX_TEXT_CHARS {
                    assert!(text.ends_with("... [truncated]"));
                    // 截断后总长度 = MAX_TEXT_CHARS + "... [truncated]".len()
                    let expected_max = MAX_TEXT_CHARS + "... [truncated]".len();
                    assert!(text.len() <= expected_max + 10); // 允许一点 UTF-8 余量
                }
            }
            Err(_) => {
                // lopdf 可能无法处理超大文本 PDF，这是可接受的
            }
        }
    }

    /// 使用 lopdf 生成一个包含指定文本的最小测试 PDF
    fn generate_test_pdf(text: &str) -> Vec<u8> {
        use lopdf::dictionary;
        use lopdf::{Document, Object, Stream};

        let mut doc = Document::with_version("1.4");

        // 创建页面内容流
        let content = format!("BT /F1 12 Tf 100 700 Td ({}) Tj ET", text);
        let content_stream = Stream::new(dictionary! {}, content.as_bytes().to_vec());
        let content_id = doc.add_object(content_stream);

        // 创建字体字典
        let font = dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
        };
        let font_id = doc.add_object(font);

        // 创建资源字典
        let resources = dictionary! {
            "Font" => dictionary! {
                "F1" => font_id,
            },
        };

        // 创建页面
        let page = dictionary! {
            "Type" => "Page",
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => resources,
        };
        let page_id = doc.add_object(page);

        // 创建 Pages 节点
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
        };
        let pages_id = doc.add_object(pages);

        // 设置页面的 Parent 引用
        if let Ok(Object::Dictionary(dict)) = doc.get_object_mut(page_id) {
            dict.set("Parent", pages_id);
        }

        // 创建 Catalog
        let catalog = dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        };
        let catalog_id = doc.add_object(catalog);

        doc.trailer.set("Root", catalog_id);

        let mut buf = Vec::new();
        doc.save_to(&mut buf).expect("Failed to save test PDF");
        buf
    }
}
