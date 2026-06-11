//! 消息内容处理（文本、图片、工具结果提取）

use crate::image::{process_gif_frames, process_image, process_image_to_format};
use crate::kiro::model::requests::conversation::KiroImage;
use crate::kiro::model::requests::tool::ToolResult;
use crate::model::config::CompressionConfig;

use super::model::ConversionError;
use crate::anthropic::types::ContentBlock;

pub(super) fn non_empty_content_or_space(content: String, has_non_text_payload: bool) -> String {
    // 尽量保留真实结构，不在早期转换阶段为非文本载荷主动补 "."。
    // 含非文本载荷时保留原始文本，最终是否需要兜底由调用方决定。
    if has_non_text_payload {
        return content;
    }
    content
}

/// 统计单个消息内容中的图片数量
pub(super) fn count_images_in_content(content: &serde_json::Value) -> usize {
    match content {
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("image"))
            .count(),
        _ => 0,
    }
}

/// 处理消息内容，提取文本、图片和工具结果
pub(super) fn process_message_content(
    content: &serde_json::Value,
    compression_config: &CompressionConfig,
    total_image_count: usize,
    remaining_image_budget: &mut usize,
) -> Result<(String, Vec<KiroImage>, Vec<ToolResult>), ConversionError> {
    let mut text_parts = Vec::new();
    let mut images = Vec::new();
    let mut tool_results = Vec::new();

    match content {
        serde_json::Value::String(s) => {
            text_parts.push(s.clone());
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if let Ok(block) = serde_json::from_value::<ContentBlock>(item.clone()) {
                    match block.block_type.as_str() {
                        "text" => {
                            if let Some(text) = block.text {
                                text_parts.push(text);
                            }
                        }
                        "image" => {
                            if let Some(source) = block.source
                                && let Some(format) = get_image_format(&source.media_type)
                            {
                                // GIF：抽帧为多张静态图，避免动图 base64 体积巨大导致上游 400
                                if format.eq_ignore_ascii_case("gif") {
                                    if *remaining_image_budget == 0 {
                                        tracing::warn!("图片配额已用尽，跳过 GIF");
                                        continue;
                                    }
                                    match process_gif_frames(
                                        &source.data,
                                        compression_config,
                                        total_image_count,
                                        *remaining_image_budget,
                                    ) {
                                        Ok(gif) => {
                                            let total_final_bytes: usize =
                                                gif.frames.iter().map(|f| f.final_bytes_len).sum();
                                            tracing::info!(
                                                duration_ms = gif.duration_ms,
                                                source_frames = gif.source_frames,
                                                sampled_frames = gif.frames.len(),
                                                sampling_interval_ms = gif.sampling_interval_ms,
                                                output_format = gif.output_format,
                                                original_bytes_len =
                                                    gif.frames[0].original_bytes_len,
                                                total_final_bytes = total_final_bytes,
                                                "GIF 已抽帧并重编码"
                                            );

                                            let frame_count = gif.frames.len();
                                            for f in gif.frames {
                                                images.push(KiroImage::from_base64(
                                                    gif.output_format,
                                                    f.data,
                                                ));
                                            }
                                            *remaining_image_budget =
                                                remaining_image_budget.saturating_sub(frame_count);
                                        }
                                        Err(e) => {
                                            tracing::warn!(
                                                "GIF 抽帧失败，回退为静态图（可能丢失动图信息）: {}",
                                                e
                                            );
                                            if *remaining_image_budget == 0 {
                                                tracing::warn!("图片配额已用尽，跳过 GIF 回退");
                                                continue;
                                            }
                                            match process_image_to_format(
                                                &source.data,
                                                "jpeg",
                                                compression_config,
                                                total_image_count,
                                            ) {
                                                Ok(result) => {
                                                    images.push(KiroImage::from_base64(
                                                        "jpeg",
                                                        result.data,
                                                    ));
                                                    *remaining_image_budget -= 1;
                                                }
                                                Err(e2) => {
                                                    tracing::warn!(
                                                        "GIF 回退重编码失败，尝试静态 GIF: {}",
                                                        e2
                                                    );
                                                    match process_image(
                                                        &source.data,
                                                        &format,
                                                        compression_config,
                                                        total_image_count,
                                                    ) {
                                                        Ok(result) => {
                                                            images.push(KiroImage::from_base64(
                                                                format,
                                                                result.data,
                                                            ));
                                                            *remaining_image_budget -= 1;
                                                        }
                                                        Err(e3) => {
                                                            tracing::warn!(
                                                                "图片处理失败，使用原始数据: {}",
                                                                e3
                                                            );
                                                            images.push(KiroImage::from_base64(
                                                                format,
                                                                source.data,
                                                            ));
                                                            *remaining_image_budget -= 1;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    // 处理静态图片（可能缩放）
                                    if *remaining_image_budget == 0 {
                                        tracing::warn!("图片配额已用尽，跳过静态图片");
                                        continue;
                                    }
                                    match process_image(
                                        &source.data,
                                        &format,
                                        compression_config,
                                        total_image_count,
                                    ) {
                                        Ok(result) => {
                                            if result.was_resized {
                                                tracing::info!(
                                                    "图片已缩放: {:?} -> {:?}, tokens: {}",
                                                    result.original_size,
                                                    result.final_size,
                                                    result.tokens
                                                );
                                            }
                                            images
                                                .push(KiroImage::from_base64(format, result.data));
                                            *remaining_image_budget -= 1;
                                        }
                                        Err(e) => {
                                            tracing::warn!("图片处理失败，使用原始数据: {}", e);
                                            images
                                                .push(KiroImage::from_base64(format, source.data));
                                            *remaining_image_budget -= 1;
                                        }
                                    }
                                }
                            }
                        }
                        "tool_result" => {
                            if let Some(tool_use_id) = block.tool_use_id {
                                let result_content = extract_tool_result_content(&block.content);
                                let is_error = block.is_error.unwrap_or(false);

                                let mut result = if is_error {
                                    ToolResult::error(&tool_use_id, result_content)
                                } else {
                                    ToolResult::success(&tool_use_id, result_content)
                                };
                                result.status =
                                    Some(if is_error { "error" } else { "success" }.to_string());

                                tool_results.push(result);
                            }
                        }
                        "tool_use" => {
                            // tool_use 在 assistant 消息中处理，这里忽略
                        }
                        "document" => {
                            // PDF 文档块：提取文本并替换为 text block
                            #[cfg(feature = "pdf-support")]
                            {
                                if let Some(ref source) = block.source
                                    && source.media_type == "application/pdf"
                                {
                                    match crate::pdf::extract_text_from_pdf(&source.data) {
                                        Ok(text) => {
                                            tracing::info!(
                                                text_chars = text.chars().count(),
                                                "PDF 文本提取成功"
                                            );
                                            text_parts.push(text);
                                        }
                                        Err(e) => {
                                            tracing::warn!("PDF 文本提取失败: {}", e);
                                            text_parts.push(crate::pdf::fallback_text(&e));
                                        }
                                    }
                                }
                            }
                            #[cfg(not(feature = "pdf-support"))]
                            {
                                // pdf-support 未启用时，document block 被静默跳过
                                tracing::debug!("跳过 document block：pdf-support feature 未启用");
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }

    Ok((text_parts.join("\n"), images, tool_results))
}

/// 从 media_type 获取图片格式
pub(super) fn get_image_format(media_type: &str) -> Option<String> {
    match media_type {
        "image/jpeg" => Some("jpeg".to_string()),
        "image/png" => Some("png".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        _ => None,
    }
}

/// 提取工具结果内容
pub(super) fn extract_tool_result_content(content: &Option<serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                }
            }
            parts.join("\n")
        }
        Some(v) => v.to_string(),
        None => String::new(),
    }
}
