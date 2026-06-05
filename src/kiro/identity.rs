//! 凭据身份特征
//!
//! 为 KiroCredentials 提供多维度身份标识：
//! - 检测指纹（设备模拟）
//! - 缓存键（domain-separated SHA-256）
//! - 凭据 ID
#![allow(dead_code)]

use sha2::{Digest, Sha256};

use crate::kiro::fingerprint::Fingerprint;
use crate::kiro::model::credentials::KiroCredentials;

/// 凭据身份特征
///
/// 提供三种身份标识方法，各自用途不同且 domain-separated：
/// - `detection_identity()` — 设备指纹，用于模拟客户端环境
/// - `cache_identity()` — 缓存键，用于 prompt cache 关联
/// - `credential_id()` — 凭据自增 ID
pub trait CredentialIdentity {
    /// 获取检测指纹（设备模拟用）
    ///
    /// 基于凭据种子生成确定性设备指纹，同一凭据始终返回相同指纹。
    /// 返回 owned Fingerprint，因为 KiroCredentials 不存储指纹。
    fn detection_identity(&self) -> Fingerprint;

    /// 获取缓存身份键（prompt cache 关联用）
    ///
    /// 使用 "cache:" domain 前缀 + 凭据种子进行 SHA-256，
    /// 确保与 detection_identity 的指纹种子 domain-separated。
    fn cache_identity(&self) -> [u8; 32];

    /// 获取凭据 ID
    fn credential_id(&self) -> u64;
}

/// 提取凭据种子
///
/// 优先级：refresh_token > kiro_api_key > machine_id > "unknown"
fn credential_seed(cred: &KiroCredentials) -> &str {
    cred.refresh_token
        .as_deref()
        .filter(|s| !s.is_empty())
        .or_else(|| cred.kiro_api_key.as_deref().filter(|s| !s.is_empty()))
        .or_else(|| cred.machine_id.as_deref().filter(|s| !s.is_empty()))
        .unwrap_or("unknown")
}

impl CredentialIdentity for KiroCredentials {
    fn detection_identity(&self) -> Fingerprint {
        let seed = credential_seed(self);
        Fingerprint::generate_from_seed(seed)
    }

    fn cache_identity(&self) -> [u8; 32] {
        let seed = credential_seed(self);
        let mut hasher = Sha256::new();
        hasher.update(b"cache:");
        hasher.update(seed.as_bytes());
        hasher.finalize().into()
    }

    fn credential_id(&self) -> u64 {
        self.id.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用凭据
    fn make_cred(refresh_token: &str, id: Option<u64>) -> KiroCredentials {
        KiroCredentials {
            id,
            refresh_token: Some(refresh_token.to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_determinism_same_credential() {
        // 同一凭据多次调用应返回相同结果
        let cred = make_cred("token-abc", Some(1));

        let fp1 = cred.detection_identity();
        let fp2 = cred.detection_identity();
        assert_eq!(fp1.machine_id, fp2.machine_id);
        assert_eq!(fp1.sdk_version, fp2.sdk_version);

        let cache1 = cred.cache_identity();
        let cache2 = cred.cache_identity();
        assert_eq!(cache1, cache2);

        assert_eq!(cred.credential_id(), 1);
    }

    #[test]
    fn test_uniqueness_different_credentials() {
        // 不同凭据应产生不同的指纹和缓存键
        let cred_a = make_cred("token-aaa", Some(1));
        let cred_b = make_cred("token-bbb", Some(2));

        let fp_a = cred_a.detection_identity();
        let fp_b = cred_b.detection_identity();
        assert_ne!(fp_a.machine_id, fp_b.machine_id);

        let cache_a = cred_a.cache_identity();
        let cache_b = cred_b.cache_identity();
        assert_ne!(cache_a, cache_b);

        assert_ne!(cred_a.credential_id(), cred_b.credential_id());
    }

    #[test]
    fn test_domain_separation() {
        // cache_identity 和 detection_identity 使用不同 domain，
        // 即使基于同一种子，产出的哈希值也不同
        let cred = make_cred("token-domain-test", Some(1));

        let fp = cred.detection_identity();
        let cache = cred.cache_identity();

        // detection_identity 的 machine_id 是 SHA-256("machine-{seed}") 的 hex，
        // cache_identity 是 SHA-256("cache:" + seed) 的 raw bytes。
        // 将 machine_id hex 解码为 bytes 后比较
        let detection_bytes = hex::decode(&fp.machine_id).unwrap();
        assert_ne!(detection_bytes.as_slice(), &cache[..]);
    }

    #[test]
    fn test_credential_id_none() {
        // id 为 None 时返回 0
        let cred = make_cred("token-x", None);
        assert_eq!(cred.credential_id(), 0);
    }

    #[test]
    fn test_seed_fallback_chain() {
        // refresh_token 为空时回退到 kiro_api_key
        let cred_api_key = KiroCredentials {
            kiro_api_key: Some("api-key-123".to_string()),
            ..Default::default()
        };
        let fp_api = cred_api_key.detection_identity();

        // 使用 machine_id 作为种子
        let cred_machine = KiroCredentials {
            machine_id: Some("machine-id-456".to_string()),
            ..Default::default()
        };
        let fp_machine = cred_machine.detection_identity();

        // 两者应不同（不同种子）
        assert_ne!(fp_api.machine_id, fp_machine.machine_id);

        // 全部为空时回退到 "unknown"
        let cred_empty = KiroCredentials::default();
        let fp_empty = cred_empty.detection_identity();
        // 应该是确定性的 "unknown" 种子
        let fp_empty2 = cred_empty.detection_identity();
        assert_eq!(fp_empty.machine_id, fp_empty2.machine_id);
    }
}
