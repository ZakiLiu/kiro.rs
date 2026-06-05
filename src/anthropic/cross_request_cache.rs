//! 跨请求缓存模块
//!
//! 实现 per-credential 的请求→conversation_id 缓存，支持 LRU 淘汰。
//! 当同一凭据发送相同消息内容时，复用上一次上游返回的 conversation_id，
//! 使得 converter 跳过自身的 conversation_id 派生逻辑（forced_conversation_id 注入）。

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

use crate::anthropic::types::Message;

/// 缓存键：(credential_id, content_fingerprint)
type CacheKey = (u64, [u8; 32]);

/// 缓存条目
#[derive(Debug, Clone)]
struct CacheEntry {
    conversation_id: String,
    #[allow(dead_code)]
    created_at: Instant,
}

/// 跨请求缓存
///
/// 使用 HashMap + VecDeque 实现 LRU 淘汰：
/// - `map` 存储 key → value 映射
/// - `order` 记录插入/访问顺序，最旧的在前
/// - 当条目数超过 `max_entries` 时，淘汰 `order` 最前面的条目
pub struct CrossRequestCache {
    inner: Mutex<LruInner>,
    max_entries: usize,
}

struct LruInner {
    map: HashMap<CacheKey, CacheEntry>,
    order: VecDeque<CacheKey>,
}

impl CrossRequestCache {
    /// 创建新的跨请求缓存
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(LruInner {
                map: HashMap::new(),
                order: VecDeque::new(),
            }),
            max_entries,
        }
    }

    /// 查找缓存的 conversation_id
    ///
    /// 如果命中，将该条目移到 LRU 队列尾部（最近使用）
    pub fn lookup(&self, credential_id: u64, fingerprint: &[u8; 32]) -> Option<String> {
        let key = (credential_id, *fingerprint);
        let mut inner = self.inner.lock();

        if let Some(entry) = inner.map.get(&key) {
            let conversation_id = entry.conversation_id.clone();
            // 移到队列尾部（最近使用）
            if let Some(pos) = inner.order.iter().position(|k| k == &key) {
                inner.order.remove(pos);
            }
            inner.order.push_back(key);
            Some(conversation_id)
        } else {
            None
        }
    }

    /// 插入缓存条目
    ///
    /// 如果 key 已存在，更新 value 并移到队列尾部。
    /// 如果超过 max_entries，淘汰最旧的条目。
    pub fn insert(&self, credential_id: u64, fingerprint: [u8; 32], conversation_id: String) {
        let key = (credential_id, fingerprint);
        let mut inner = self.inner.lock();

        if inner.map.contains_key(&key) {
            // 已存在：更新并移到尾部
            inner.map.insert(
                key,
                CacheEntry {
                    conversation_id,
                    created_at: Instant::now(),
                },
            );
            if let Some(pos) = inner.order.iter().position(|k| k == &key) {
                inner.order.remove(pos);
            }
            inner.order.push_back(key);
        } else {
            // 新条目：先淘汰（如果满了）
            while inner.map.len() >= self.max_entries {
                if let Some(oldest_key) = inner.order.pop_front() {
                    inner.map.remove(&oldest_key);
                } else {
                    break;
                }
            }
            inner.map.insert(
                key,
                CacheEntry {
                    conversation_id,
                    created_at: Instant::now(),
                },
            );
            inner.order.push_back(key);
        }
    }

    /// 计算消息内容的 SHA-256 指纹
    ///
    /// 将所有消息的 role + content 序列化后计算 SHA-256，
    /// 作为缓存 key 的一部分。
    pub fn content_fingerprint(messages: &[Message]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        for msg in messages {
            hasher.update(msg.role.as_bytes());
            hasher.update(b"\x1f"); // unit separator
            if let Some(s) = msg.content.as_str() {
                hasher.update(s.as_bytes());
            } else {
                let s = serde_json::to_string(&msg.content).unwrap_or_default();
                hasher.update(s.as_bytes());
            }
            hasher.update(b"\x1e"); // record separator
        }
        hasher.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_messages(texts: &[&str]) -> Vec<Message> {
        texts
            .iter()
            .enumerate()
            .map(|(i, text)| Message {
                role: if i % 2 == 0 {
                    "user".to_string()
                } else {
                    "assistant".to_string()
                },
                content: serde_json::Value::String(text.to_string()),
            })
            .collect()
    }

    #[test]
    fn test_lookup_miss() {
        let cache = CrossRequestCache::new(10);
        let fp = CrossRequestCache::content_fingerprint(&make_messages(&["hello"]));
        assert!(cache.lookup(1, &fp).is_none());
    }

    #[test]
    fn test_insert_and_lookup_hit() {
        let cache = CrossRequestCache::new(10);
        let fp = CrossRequestCache::content_fingerprint(&make_messages(&["hello"]));
        cache.insert(1, fp, "conv-123".to_string());

        let result = cache.lookup(1, &fp);
        assert_eq!(result, Some("conv-123".to_string()));
    }

    #[test]
    fn test_lru_eviction() {
        let cache = CrossRequestCache::new(3);

        let fp1 = CrossRequestCache::content_fingerprint(&make_messages(&["msg1"]));
        let fp2 = CrossRequestCache::content_fingerprint(&make_messages(&["msg2"]));
        let fp3 = CrossRequestCache::content_fingerprint(&make_messages(&["msg3"]));
        let fp4 = CrossRequestCache::content_fingerprint(&make_messages(&["msg4"]));

        cache.insert(1, fp1, "conv-1".to_string());
        cache.insert(1, fp2, "conv-2".to_string());
        cache.insert(1, fp3, "conv-3".to_string());

        // 缓存满了，插入第 4 个应该淘汰第 1 个
        cache.insert(1, fp4, "conv-4".to_string());

        assert!(cache.lookup(1, &fp1).is_none(), "fp1 should be evicted");
        assert_eq!(cache.lookup(1, &fp2), Some("conv-2".to_string()));
        assert_eq!(cache.lookup(1, &fp3), Some("conv-3".to_string()));
        assert_eq!(cache.lookup(1, &fp4), Some("conv-4".to_string()));
    }

    #[test]
    fn test_lru_access_refreshes_order() {
        let cache = CrossRequestCache::new(3);

        let fp1 = CrossRequestCache::content_fingerprint(&make_messages(&["msg1"]));
        let fp2 = CrossRequestCache::content_fingerprint(&make_messages(&["msg2"]));
        let fp3 = CrossRequestCache::content_fingerprint(&make_messages(&["msg3"]));
        let fp4 = CrossRequestCache::content_fingerprint(&make_messages(&["msg4"]));

        cache.insert(1, fp1, "conv-1".to_string());
        cache.insert(1, fp2, "conv-2".to_string());
        cache.insert(1, fp3, "conv-3".to_string());

        // 访问 fp1，使其变为最近使用
        cache.lookup(1, &fp1);

        // 插入 fp4 应该淘汰 fp2（最久未使用），而非 fp1
        cache.insert(1, fp4, "conv-4".to_string());

        assert_eq!(cache.lookup(1, &fp1), Some("conv-1".to_string()));
        assert!(cache.lookup(1, &fp2).is_none(), "fp2 should be evicted");
        assert_eq!(cache.lookup(1, &fp3), Some("conv-3".to_string()));
        assert_eq!(cache.lookup(1, &fp4), Some("conv-4".to_string()));
    }

    #[test]
    fn test_per_credential_isolation() {
        let cache = CrossRequestCache::new(10);
        let fp = CrossRequestCache::content_fingerprint(&make_messages(&["hello"]));

        cache.insert(1, fp, "conv-cred1".to_string());
        cache.insert(2, fp, "conv-cred2".to_string());

        assert_eq!(cache.lookup(1, &fp), Some("conv-cred1".to_string()));
        assert_eq!(cache.lookup(2, &fp), Some("conv-cred2".to_string()));
        // 不同凭据的同指纹互不干扰
        assert!(cache.lookup(3, &fp).is_none());
    }

    #[test]
    fn test_content_fingerprint_determinism() {
        let msgs = make_messages(&["hello", "world"]);
        let fp1 = CrossRequestCache::content_fingerprint(&msgs);
        let fp2 = CrossRequestCache::content_fingerprint(&msgs);
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_content_fingerprint_differs_for_different_content() {
        let msgs1 = make_messages(&["hello"]);
        let msgs2 = make_messages(&["world"]);
        let fp1 = CrossRequestCache::content_fingerprint(&msgs1);
        let fp2 = CrossRequestCache::content_fingerprint(&msgs2);
        assert_ne!(fp1, fp2);
    }

    #[test]
    fn test_insert_updates_existing() {
        let cache = CrossRequestCache::new(10);
        let fp = CrossRequestCache::content_fingerprint(&make_messages(&["hello"]));

        cache.insert(1, fp, "conv-old".to_string());
        cache.insert(1, fp, "conv-new".to_string());

        assert_eq!(cache.lookup(1, &fp), Some("conv-new".to_string()));

        // 更新不应增加条目数
        let inner = cache.inner.lock();
        assert_eq!(inner.map.len(), 1);
        assert_eq!(inner.order.len(), 1);
    }
}
