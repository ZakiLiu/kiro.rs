//! Token 使用事件 (tokenUsageEvent)
//!
//! Kiro 后端在流末端下发精确 token 计量，包含输入/输出/缓存明细。
//! 此前被当 Unknown 丢弃，导致只能用本地估算。

use serde::Deserialize;

use crate::kiro::parser::error::ParseResult;
use crate::kiro::parser::frame::Frame;

use super::base::EventPayload;

#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsageEvent {
    #[serde(default)]
    pub uncached_input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: Option<i64>,
    #[serde(default)]
    pub cache_write_input_tokens: Option<i64>,
}

impl EventPayload for TokenUsageEvent {
    fn from_frame(frame: &Frame) -> ParseResult<Self> {
        match frame.payload_as_json::<Self>() {
            Ok(ev) => Ok(ev),
            Err(_) => Ok(Self::default()),
        }
    }
}

fn clamp_i32(v: i64) -> i32 {
    v.clamp(0, i32::MAX as i64) as i32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BillingSplit {
    pub input_tokens: i32,
    pub cache_creation_input_tokens: i32,
    pub cache_read_input_tokens: i32,
    pub output_tokens: i32,
}

impl TokenUsageEvent {
    pub fn billing_split(&self) -> BillingSplit {
        let uncached = self.uncached_input_tokens.max(0);
        let cache_read = self.cache_read_input_tokens.unwrap_or(0).max(0);
        let cache_write = self.cache_write_input_tokens.unwrap_or(0).max(0);
        let output = self.output_tokens.max(0);

        let fresh_input = if cache_write <= uncached {
            uncached - cache_write
        } else {
            uncached
        };

        BillingSplit {
            input_tokens: clamp_i32(fresh_input),
            cache_creation_input_tokens: clamp_i32(cache_write),
            cache_read_input_tokens: clamp_i32(cache_read),
            output_tokens: clamp_i32(output),
        }
    }

    pub fn has_real_usage(&self) -> bool {
        self.uncached_input_tokens > 0
            || self.output_tokens > 0
            || self.total_tokens > 0
            || self.cache_read_input_tokens.unwrap_or(0) > 0
            || self.cache_write_input_tokens.unwrap_or(0) > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_camelcase() {
        let json = r#"{"uncachedInputTokens":12345,"outputTokens":678,"totalTokens":13023,"cacheReadInputTokens":100,"cacheWriteInputTokens":50}"#;
        let ev: TokenUsageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.uncached_input_tokens, 12345);
        assert_eq!(ev.output_tokens, 678);
        assert_eq!(ev.total_tokens, 13023);
        assert_eq!(ev.cache_read_input_tokens, Some(100));
        assert_eq!(ev.cache_write_input_tokens, Some(50));
    }

    #[test]
    fn billing_split_subtracts_write() {
        let ev = TokenUsageEvent {
            uncached_input_tokens: 1000,
            output_tokens: 200,
            total_tokens: 1500,
            cache_read_input_tokens: Some(300),
            cache_write_input_tokens: Some(400),
        };
        let s = ev.billing_split();
        assert_eq!(s.input_tokens, 600);
        assert_eq!(s.cache_creation_input_tokens, 400);
        assert_eq!(s.cache_read_input_tokens, 300);
        assert_eq!(s.output_tokens, 200);
    }

    #[test]
    fn billing_split_guard_write_exceeds_uncached() {
        let ev = TokenUsageEvent {
            uncached_input_tokens: 100,
            output_tokens: 10,
            total_tokens: 110,
            cache_read_input_tokens: None,
            cache_write_input_tokens: Some(500),
        };
        let s = ev.billing_split();
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.cache_creation_input_tokens, 500);
    }

    #[test]
    fn missing_cache_fields() {
        let json = r#"{"uncachedInputTokens":100,"outputTokens":20,"totalTokens":120}"#;
        let ev: TokenUsageEvent = serde_json::from_str(json).unwrap();
        assert_eq!(ev.cache_read_input_tokens, None);
        assert!(ev.has_real_usage());
    }

    #[test]
    fn empty_not_real() {
        assert!(!TokenUsageEvent::default().has_real_usage());
    }
}
