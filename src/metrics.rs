//! 请求生命周期指标收集器
//!
//! 使用固定大小环形缓冲区（`VecDeque`）记录请求事件，
//! 超出容量时自动淘汰最旧条目。

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::Serialize;

/// 指标事件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricEventType {
    /// 收到请求
    RequestReceived,
    /// 选择凭据
    CredentialSelected,
    /// 请求完成
    RequestCompleted,
}

/// 单条指标事件
#[derive(Debug, Clone, Serialize)]
pub struct MetricEvent {
    /// 事件发生时间
    pub timestamp: DateTime<Utc>,
    /// 事件类型
    pub event_type: MetricEventType,
    /// 模型名称
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 凭据 ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<u64>,
    /// 请求延迟（毫秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// 状态（success / failure / error）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// 输入 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<i32>,
    /// 输出 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<i32>,
}

impl MetricEvent {
    /// 快速构造指定类型的事件（时间戳自动填充为当前 UTC 时间）
    pub fn new(event_type: MetricEventType) -> Self {
        Self {
            timestamp: Utc::now(),
            event_type,
            model: None,
            credential_id: None,
            latency_ms: None,
            status: None,
            input_tokens: None,
            output_tokens: None,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_status(mut self, status: impl Into<String>) -> Self {
        self.status = Some(status.into());
        self
    }

    pub fn with_latency_ms(mut self, ms: u64) -> Self {
        self.latency_ms = Some(ms);
        self
    }

    pub fn with_tokens(mut self, input: i32, output: i32) -> Self {
        self.input_tokens = Some(input);
        self.output_tokens = Some(output);
        self
    }
}

/// 指标收集器——固定大小环形缓冲区
pub struct MetricsCollector {
    buffer: Mutex<VecDeque<MetricEvent>>,
    max_size: usize,
}

impl MetricsCollector {
    /// 创建新的指标收集器
    ///
    /// `max_size` 为环形缓冲区最大容量，超出后自动淘汰最旧条目。
    pub fn new(max_size: usize) -> Self {
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(max_size.min(65536))),
            max_size,
        }
    }

    /// 记录一条事件（fire-and-forget，持锁时间极短）
    pub fn record(&self, event: MetricEvent) {
        let mut buf = self.buffer.lock();
        if buf.len() >= self.max_size {
            buf.pop_front();
        }
        buf.push_back(event);
    }

    /// 返回当前缓冲区的快照（克隆）
    pub fn snapshot(&self) -> Vec<MetricEvent> {
        let buf = self.buffer.lock();
        buf.iter().cloned().collect()
    }

    /// 返回缓冲区当前事件数量
    pub fn len(&self) -> usize {
        self.buffer.lock().len()
    }

    /// 缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.buffer.lock().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_snapshot() {
        let collector = MetricsCollector::new(100);
        assert!(collector.is_empty());

        collector.record(MetricEvent::new(MetricEventType::RequestReceived).with_model("claude-sonnet-4-6"));
        collector.record(MetricEvent::new(MetricEventType::RequestCompleted).with_status("success"));

        let snap = collector.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].event_type, MetricEventType::RequestReceived);
        assert_eq!(snap[0].model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(snap[1].event_type, MetricEventType::RequestCompleted);
        assert_eq!(snap[1].status.as_deref(), Some("success"));
    }

    #[test]
    fn test_ring_buffer_evicts_oldest() {
        let collector = MetricsCollector::new(3);

        for i in 0..5 {
            let mut event = MetricEvent::new(MetricEventType::RequestReceived);
            event.input_tokens = Some(i);
            collector.record(event);
        }

        assert_eq!(collector.len(), 3);
        let snap = collector.snapshot();
        // 应该保留最后 3 条（input_tokens 2, 3, 4）
        assert_eq!(snap[0].input_tokens, Some(2));
        assert_eq!(snap[1].input_tokens, Some(3));
        assert_eq!(snap[2].input_tokens, Some(4));
    }

    #[test]
    fn test_snapshot_returns_clone() {
        let collector = MetricsCollector::new(10);
        collector.record(MetricEvent::new(MetricEventType::RequestReceived));

        let snap1 = collector.snapshot();
        collector.record(MetricEvent::new(MetricEventType::RequestCompleted));
        let snap2 = collector.snapshot();

        // snap1 不受后续 record 影响
        assert_eq!(snap1.len(), 1);
        assert_eq!(snap2.len(), 2);
    }

    #[test]
    fn test_empty_collector() {
        let collector = MetricsCollector::new(10);
        assert!(collector.is_empty());
        assert_eq!(collector.len(), 0);
        assert!(collector.snapshot().is_empty());
    }

    #[test]
    fn test_max_size_one() {
        let collector = MetricsCollector::new(1);
        collector.record(MetricEvent::new(MetricEventType::RequestReceived));
        collector.record(MetricEvent::new(MetricEventType::RequestCompleted));

        assert_eq!(collector.len(), 1);
        let snap = collector.snapshot();
        assert_eq!(snap[0].event_type, MetricEventType::RequestCompleted);
    }

    #[test]
    fn test_event_builder_methods() {
        let event = MetricEvent::new(MetricEventType::RequestCompleted)
            .with_model("claude-opus-4-6")
            .with_status("success")
            .with_latency_ms(150)
            .with_tokens(1000, 500);

        assert_eq!(event.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(event.status.as_deref(), Some("success"));
        assert_eq!(event.latency_ms, Some(150));
        assert_eq!(event.input_tokens, Some(1000));
        assert_eq!(event.output_tokens, Some(500));
    }
}
