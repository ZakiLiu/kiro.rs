//! Token 管理模块
//!
//! 负责 Token 过期检测和刷新，支持 Social 和 IdC 认证方式
//! 支持单凭据 (TokenManager) 和多凭据 (MultiTokenManager) 管理
//!
//! ## 增强特性
//!
//! - **多维度设备指纹**: 每个凭据生成独立的设备指纹，模拟真实客户端
//! - **后台 Token 刷新**: 定期检查并预刷新即将过期的 Token
//! - **精细化速率限制**: 每日请求限制、请求间隔控制、指数退避
//! - **冷却管理**: 分类管理不同原因的冷却状态
//! - **优雅降级**: Token 刷新失败时使用现有 Token

pub(crate) mod balance;
pub(crate) mod multi;
pub(crate) mod refresh;
pub(crate) mod single;
pub(crate) mod types;

// Re-export public API
pub use multi::{CallContext, MultiTokenManager};
pub use types::{CachedBalanceInfo, DisableReason};

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::multi::MAX_FAILURES_PER_CREDENTIAL;
    use super::refresh::{build_idc_refresh_user_agents, sha256_hex};
    use super::single::{
        TokenManager, is_token_expired, is_token_expiring_soon, validate_refresh_token,
    };
    use super::*;
    use crate::kiro::cooldown::CooldownReason;
    use crate::kiro::endpoint::{IdeEndpoint, KiroEndpoint, RequestContext};
    use crate::kiro::model::credentials::KiroCredentials;
    use crate::model::config::Config;
    use chrono::{Duration, Utc};

    #[test]
    fn test_token_manager_new() {
        let config = Config::default();
        let credentials = KiroCredentials::default();
        let tm = TokenManager::new(config, credentials, None);
        assert!(tm.credentials().access_token.is_none());
    }

    #[test]
    fn test_is_token_expired_with_expired_token() {
        let mut credentials = KiroCredentials::default();
        credentials.expires_at = Some("2020-01-01T00:00:00Z".to_string());
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_with_valid_token() {
        let mut credentials = KiroCredentials::default();
        let future = Utc::now() + Duration::hours(1);
        credentials.expires_at = Some(future.to_rfc3339());
        assert!(!is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_within_5_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(3);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expired_no_expires_at() {
        let credentials = KiroCredentials::default();
        assert!(is_token_expired(&credentials));
    }

    #[test]
    fn test_is_token_expiring_soon_within_10_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(8);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(is_token_expiring_soon(&credentials));
    }

    #[test]
    fn test_is_token_expiring_soon_beyond_10_minutes() {
        let mut credentials = KiroCredentials::default();
        let expires = Utc::now() + Duration::minutes(15);
        credentials.expires_at = Some(expires.to_rfc3339());
        assert!(!is_token_expiring_soon(&credentials));
    }

    #[test]
    fn test_validate_refresh_token_missing() {
        let credentials = KiroCredentials::default();
        let result = validate_refresh_token(&credentials);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_refresh_token_valid() {
        let mut credentials = KiroCredentials::default();
        credentials.refresh_token = Some("a".repeat(150));
        let result = validate_refresh_token(&credentials);
        assert!(result.is_ok());
    }

    #[test]
    fn test_sha256_hex() {
        let result = sha256_hex("test");
        assert_eq!(
            result,
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
        );
    }

    #[tokio::test]
    async fn test_add_credential_reject_duplicate_refresh_token() {
        let config = Config::default();

        let mut existing = KiroCredentials::default();
        existing.refresh_token = Some("a".repeat(150));

        let manager = MultiTokenManager::new(config, vec![existing], None, None, false).unwrap();

        let mut duplicate = KiroCredentials::default();
        duplicate.refresh_token = Some("a".repeat(150));

        let result = manager.add_credential(duplicate).await;
        assert!(result.is_err());
        assert!(result.err().unwrap().to_string().contains("凭据已存在"));
    }

    // MultiTokenManager 测试

    #[test]
    fn test_multi_token_manager_new() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.priority = 0;
        let mut cred2 = KiroCredentials::default();
        cred2.priority = 1;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();
        assert_eq!(manager.total_count(), 2);
        assert_eq!(manager.available_count(), 2);
    }

    #[test]
    fn test_invalidate_access_token_marks_expired() {
        let config = Config::default();
        let mut credentials = KiroCredentials::default();
        credentials.refresh_token = Some("a".repeat(150));
        credentials.access_token = Some("some_token".to_string());
        credentials.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![credentials], None, None, false).unwrap();
        assert!(manager.invalidate_access_token(1));

        let snapshot = manager.snapshot();
        let entry = snapshot.entries.iter().find(|e| e.id == 1).unwrap();
        let mut cred = KiroCredentials::default();
        cred.expires_at = entry.expires_at.clone();
        assert!(is_token_expired(&cred));
    }

    #[test]
    fn test_multi_token_manager_empty_credentials() {
        let config = Config::default();
        let result = MultiTokenManager::new(config, vec![], None, None, false);
        // 支持 0 个凭据启动（可通过管理面板添加）
        assert!(result.is_ok());
        let manager = result.unwrap();
        assert_eq!(manager.total_count(), 0);
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_duplicate_ids() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.id = Some(1);
        let mut cred2 = KiroCredentials::default();
        cred2.id = Some(1); // 重复 ID

        let result = MultiTokenManager::new(config, vec![cred1, cred2], None, None, false);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("重复的凭据 ID"),
            "错误消息应包含 '重复的凭据 ID'，实际: {}",
            err_msg
        );
    }

    #[test]
    fn test_multi_token_manager_report_failure() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        // MAX_FAILURES_PER_CREDENTIAL = 3，所以前两次失败不会禁用
        assert!(manager.report_failure(1));
        assert_eq!(manager.available_count(), 2);
        assert!(manager.report_failure(1));
        assert_eq!(manager.available_count(), 2);

        // 第三次失败会禁用第一个凭据
        assert!(manager.report_failure(1));
        assert_eq!(manager.available_count(), 1);

        // 继续失败第二个凭据（使用 ID 2），需要 3 次才会禁用
        assert!(manager.report_failure(2));
        assert!(manager.report_failure(2));
        assert!(!manager.report_failure(2)); // 所有凭据都禁用了
        assert_eq!(manager.available_count(), 0);
    }

    #[test]
    fn test_multi_token_manager_report_success() {
        let config = Config::default();
        let cred = KiroCredentials::default();

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        // 失败一次（使用 ID 1）
        manager.report_failure(1);

        // 成功后重置计数（使用 ID 1）
        manager.report_success(1);

        // 再失败一次不会禁用（因为计数已重置）
        manager.report_failure(1);
        assert_eq!(manager.available_count(), 1);
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_auto_recovers_all_disabled() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(1);
        }
        for _ in 0..MAX_FAILURES_PER_CREDENTIAL {
            manager.report_failure(2);
        }

        assert_eq!(manager.available_count(), 0);

        // 应触发自愈：重置失败计数并重新启用，避免必须重启进程
        let ctx = manager.acquire_context().await.unwrap();
        assert!(ctx.token == "t1" || ctx.token == "t2");
        assert_eq!(manager.available_count(), 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_prefers_higher_balance_when_usage_equal() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 两个凭据使用次数都为 0 时，应优先选择余额更高的
        manager.update_balance_cache(1, 100.0);
        manager.update_balance_cache(2, 200.0);

        let ctx = manager.acquire_context().await.unwrap();
        assert_eq!(ctx.id, 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_round_robin_when_balance_and_usage_equal() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        manager.update_balance_cache(1, 100.0);
        manager.update_balance_cache(2, 100.0);

        let ctx1 = manager.acquire_context().await.unwrap();
        let ctx2 = manager.acquire_context().await.unwrap();
        assert_ne!(ctx1.id, ctx2.id);
    }

    #[test]
    fn test_multi_token_manager_report_quota_exhausted() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 凭据会自动分配 ID（从 1 开始）
        assert_eq!(manager.available_count(), 2);
        assert!(manager.report_quota_exhausted(1));
        assert_eq!(manager.available_count(), 1);

        // 再禁用第二个后，无可用凭据
        assert!(!manager.report_quota_exhausted(2));
        assert_eq!(manager.available_count(), 0);
    }

    #[tokio::test]
    async fn test_multi_token_manager_quota_disabled_is_not_auto_recovered() {
        let config = Config::default();
        let cred1 = KiroCredentials::default();
        let cred2 = KiroCredentials::default();

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        manager.report_quota_exhausted(1);
        manager.report_quota_exhausted(2);
        assert_eq!(manager.available_count(), 0);

        let err = manager.acquire_context().await.err().unwrap().to_string();
        assert!(
            err.contains("所有凭据均已禁用"),
            "错误应提示所有凭据禁用，实际: {}",
            err
        );
        assert_eq!(manager.available_count(), 0);
    }

    /// recovery 循环 Ok 分支语义：探测成功即复活，余额 0 如实写入缓存，
    /// 复活后凭据对 acquire_context 可见（余额仅参与 LB 评分排序，不做过滤）
    #[tokio::test]
    async fn test_multi_token_manager_quota_disabled_recovered_with_zero_balance() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred1], None, None, false).unwrap();

        manager.report_quota_exhausted(1);
        assert_eq!(manager.available_count(), 0);

        // 模拟 recovery 循环 Ok 分支：探测成功即复活，余额 0 如实写入
        assert!(manager.recover_credential_inner(1));
        manager.update_balance_cache(1, 0.0);

        assert_eq!(manager.available_count(), 1);
        let ctx = manager.acquire_context().await.unwrap();
        assert_eq!(ctx.id, 1);
        assert_eq!(ctx.token, "t1");
    }

    #[tokio::test]
    async fn test_multi_token_manager_rate_limited_with_some_disabled_does_not_report_all_disabled()
    {
        // 复现线上日志：
        // - total > available（部分凭据被禁用）
        // - 所有可用凭据都被速率限制/冷却暂时挡住
        // 期望：等待最短可用时间后继续尝试，而不是误报“所有凭据均已禁用（x/y）”。

        let mut config = Config::default();
        // 固定间隔 10ms，避免测试过慢且消除抖动带来的不确定性
        config.credential_rpm = Some(6000);

        let cred1 = KiroCredentials {
            access_token: Some("token-1".to_string()),
            expires_at: Some("2999-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };
        let cred2 = KiroCredentials {
            access_token: Some("token-2".to_string()),
            expires_at: Some("2999-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 禁用 #2，仅保留一个可用凭据
        assert!(manager.report_quota_exhausted(2));
        assert_eq!(manager.available_count(), 1);

        // 预先占位：让 #1 在下一次 acquire_context() 时必然触发速率限制
        assert!(manager.rate_limiter().try_acquire(1).is_ok());

        // 关键断言：不会抛出“所有凭据均已禁用（1/2）”，而是等待后成功返回。
        let ctx = manager.acquire_context().await.unwrap();
        assert_eq!(ctx.id, 1);
    }

    #[tokio::test]
    async fn test_credential_rpm_zero_disables_local_rate_limiter_and_default_daily_cap() {
        let mut config = Config::default();
        config.credential_rpm = Some(0);
        // 不显式配置 daily cap；credentialRpm=0 应同时关闭默认 daily cap。
        config.credential_daily_max_requests = None;

        let cred = KiroCredentials {
            access_token: Some("token-1".to_string()),
            expires_at: Some("2999-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        for _ in 0..=crate::kiro::rate_limiter::DEFAULT_DAILY_MAX_REQUESTS {
            let ctx = manager.acquire_context().await.unwrap();
            assert_eq!(ctx.id, 1);
            manager.record_api_success(ctx.id);
        }
    }

    #[tokio::test]
    async fn test_credential_daily_max_requests_can_override_rpm_zero() {
        let mut config = Config::default();
        config.credential_rpm = Some(0);
        config.credential_daily_max_requests = Some(2);

        let cred = KiroCredentials {
            access_token: Some("token-1".to_string()),
            expires_at: Some("2999-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        for _ in 0..2 {
            let ctx = manager.acquire_context().await.unwrap();
            manager.record_api_success(ctx.id);
        }

        let err = manager.acquire_context().await.err().unwrap();
        assert!(err.to_string().contains("所有凭据均处于冷却/速率限制"));
        assert!(err.to_string().contains("原因：rate_limit"));
    }

    /// 组合 setter 热更新：单次调用同时生效 RPM 与每日上限（锁内原子发布）。
    #[tokio::test]
    async fn test_update_rate_limit_settings_hot_applies_combined_config() {
        let config = Config::default();

        let cred = KiroCredentials {
            access_token: Some("token-1".to_string()),
            expires_at: Some("2999-01-01T00:00:00Z".to_string()),
            ..Default::default()
        };

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        // 热更新：关闭本地 RPM 节流 + daily cap = 2
        manager.update_rate_limit_settings(Some(0), Some(2));

        for _ in 0..2 {
            let ctx = manager.acquire_context().await.unwrap();
            manager.record_api_success(ctx.id);
        }

        let err = manager.acquire_context().await.err().unwrap();
        assert!(err.to_string().contains("所有凭据均处于冷却/速率限制"));
        assert!(err.to_string().contains("原因：rate_limit"));
    }

    #[test]
    fn test_set_credential_cooldown_with_duration_does_not_increment_failure_count() {
        let config = Config::default();
        let manager =
            MultiTokenManager::new(config, vec![KiroCredentials::default()], None, None, false)
                .unwrap();

        let cooldown = manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_secs(120)),
        );
        assert_eq!(cooldown, std::time::Duration::from_secs(120));

        let snapshot = manager.snapshot();
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].failure_count, 0);
        assert!(!snapshot.entries[0].disabled);
        assert!(snapshot.entries[0].last_used_at.is_some());

        let (reason, remaining) = manager.cooldown_manager().check_cooldown(1).unwrap();
        assert_eq!(reason, CooldownReason::RateLimitExceeded);
        assert!(remaining <= std::time::Duration::from_secs(120));
        assert!(remaining > std::time::Duration::from_secs(100));
    }

    #[tokio::test]
    async fn test_report_account_suspended_sets_cooldown_and_skips_acquire() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred1.priority = 0;

        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred2.priority = 0;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        let duration = manager.report_account_suspended(1);
        assert!(duration > std::time::Duration::ZERO);

        // 进入 AccountSuspended 冷却（而非禁用）
        let (reason, remaining) = manager.cooldown_manager().check_cooldown(1).unwrap();
        assert_eq!(reason, CooldownReason::AccountSuspended);
        assert!(remaining > std::time::Duration::ZERO);

        let snapshot = manager.snapshot();
        let entry1 = snapshot.entries.iter().find(|e| e.id == 1).unwrap();
        assert!(!entry1.disabled);

        // acquire 跳过冷却中的凭据 1，选中凭据 2
        let ctx = manager.acquire_context().await.unwrap();
        assert_eq!(ctx.id, 2);
    }

    #[tokio::test]
    async fn test_account_suspended_cooldown_expires_and_recovers() {
        let config = Config::default();
        let mut cred = KiroCredentials::default();
        cred.access_token = Some("t1".to_string());
        cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::AccountSuspended,
            Some(std::time::Duration::from_millis(50)),
        );
        assert!(manager.cooldown_manager().check_cooldown(1).is_some());

        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        // 到期自动回池，无需人工清除
        assert!(manager.cooldown_manager().check_cooldown(1).is_none());
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_skips_rate_limited_credential() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred1.priority = 0;

        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred2.priority = 0;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_millis(200)),
        );

        let ctx = manager.acquire_context().await.unwrap();
        assert_eq!(ctx.id, 2);
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_waits_until_rate_limit_cooldown_expires() {
        let config = Config::default();
        let mut cred = KiroCredentials::default();
        cred.access_token = Some("t1".to_string());
        cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_millis(150)),
        );

        let started = std::time::Instant::now();
        let ctx = manager.acquire_context().await.unwrap();
        let elapsed = started.elapsed();

        assert_eq!(ctx.id, 1);
        assert!(elapsed >= std::time::Duration::from_millis(120));
    }

    #[tokio::test]
    async fn test_acquire_context_bails_when_short_cooling_waits_exceed_budget() {
        let config = Config::default();
        let mut cred = KiroCredentials::default();
        cred.access_token = Some("t1".to_string());
        cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = std::sync::Arc::new(
            MultiTokenManager::new(config, vec![cred], None, None, false).unwrap(),
        );

        // 单次等待低于 2s bail 阈值：第一轮应允许短睡。
        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_millis(220)),
        );

        // 在 acquire 短睡期间持续续上冷却，模拟线上高并发下“每轮 wait 都不长，
        // 但总等待不断滚动延长”的状态。
        let extender = std::sync::Arc::clone(&manager);
        let extend_task = tokio::spawn(async move {
            for _ in 0..6 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                extender.set_credential_cooldown_with_duration(
                    1,
                    CooldownReason::RateLimitExceeded,
                    Some(std::time::Duration::from_millis(220)),
                );
            }
        });

        let started = std::time::Instant::now();
        let err = manager.acquire_context().await.err().unwrap();
        let elapsed = started.elapsed();
        extend_task.await.unwrap();

        assert!(
            elapsed < std::time::Duration::from_millis(700),
            "应在累计等待预算耗尽后快速返回，实际耗时: {:?}",
            elapsed
        );
        assert!(err.to_string().contains("所有凭据均处于冷却/速率限制"));
        assert!(err.to_string().contains("retry_after_secs="));
    }

    #[tokio::test]
    async fn test_acquire_context_preserves_exclusions_while_waiting_for_cooling_budget() {
        let config = Config::default();
        let mut excluded_cred = KiroCredentials::default();
        excluded_cred.access_token = Some("excluded-token".to_string());
        excluded_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        excluded_cred.priority = 0;

        let mut cooling_cred = KiroCredentials::default();
        cooling_cred.access_token = Some("cooling-token".to_string());
        cooling_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cooling_cred.priority = 0;

        let manager = std::sync::Arc::new(
            MultiTokenManager::new(config, vec![excluded_cred, cooling_cred], None, None, false)
                .unwrap(),
        );

        manager.set_credential_cooldown_with_duration(
            2,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_millis(220)),
        );

        // 持续续上 #2 的短冷却；如果 sleep 后丢失 exclude_ids，旧逻辑会错误返回 #1。
        let extender = std::sync::Arc::clone(&manager);
        let extend_task = tokio::spawn(async move {
            for _ in 0..6 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                extender.set_credential_cooldown_with_duration(
                    2,
                    CooldownReason::RateLimitExceeded,
                    Some(std::time::Duration::from_millis(220)),
                );
            }
        });

        let err = manager.acquire_context_excluding(&[1]).await.err().unwrap();
        extend_task.await.unwrap();

        assert!(err.to_string().contains("所有凭据均处于冷却/速率限制"));
        assert!(err.to_string().contains("retry_after_secs="));
    }

    #[tokio::test]
    async fn test_acquire_context_excluding_ignores_disabled_exclusions_for_exhaustion_count() {
        let config = Config::default();
        let mut disabled_cred = KiroCredentials::default();
        disabled_cred.access_token = Some("disabled-token".to_string());
        disabled_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let mut enabled_cred = KiroCredentials::default();
        enabled_cred.access_token = Some("enabled-token".to_string());
        enabled_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager =
            MultiTokenManager::new(config, vec![disabled_cred, enabled_cred], None, None, false)
                .unwrap();

        assert!(manager.report_quota_exhausted(1));
        assert_eq!(manager.available_count(), 1);

        let ctx = manager.acquire_context_excluding(&[1]).await.unwrap();
        assert_eq!(ctx.id, 2);
    }

    #[tokio::test]
    async fn test_acquire_context_bails_when_all_credentials_cooling_longer_than_threshold() {
        let config = Config::default();
        let mut cred = KiroCredentials::default();
        cred.access_token = Some("t1".to_string());
        cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());

        let manager = MultiTokenManager::new(config, vec![cred], None, None, false).unwrap();

        // 设置 10 秒冷却，超过 2 秒阈值
        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_secs(10)),
        );

        let started = std::time::Instant::now();
        let err = manager.acquire_context().await.err().unwrap();
        let elapsed = started.elapsed();

        // 应立即返回错误，不会长睡
        assert!(elapsed < std::time::Duration::from_secs(2));
        assert!(err.to_string().contains("所有凭据均处于冷却/速率限制"));
        assert!(err.to_string().contains("retry_after_secs="));
    }

    #[tokio::test]
    async fn test_acquire_context_bails_with_total_exhausted_branch() {
        let config = Config::default();
        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred1.priority = 0;

        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred2.priority = 1; // 不同优先级，确保两个都被尝试

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 两个凭据都设置长冷却
        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_secs(10)),
        );
        manager.set_credential_cooldown_with_duration(
            2,
            CooldownReason::ServerError,
            Some(std::time::Duration::from_secs(10)),
        );

        let started = std::time::Instant::now();
        let err = manager.acquire_context().await.err().unwrap();
        let elapsed = started.elapsed();

        assert!(elapsed < std::time::Duration::from_secs(2));
        assert!(err.to_string().contains("所有凭据均处于冷却/速率限制"));
        assert!(err.to_string().contains("retry_after_secs="));
    }

    /// MNT-001 重构 divergence guard：enabled-exhausted 与 total-exhausted 两个穷尽分支
    /// 在“全部因长冷却退出”这一相同输入下，必须产出逐字一致的 bail 错误（同一个 429 文案）。
    ///
    /// - total-exhausted：单个长冷却凭据（id=1），tried_ids 覆盖 total。
    /// - enabled-exhausted：长冷却凭据（id=1）+ 一个 quota 禁用凭据（id=2），
    ///   仅启用集合被尝试完、total 未覆盖。
    ///
    /// 两者的最短等待都来自同一个凭据 #1，冷却时长一致（且远大于秒级，规避
    /// Retry-After 向上取整在两次 acquire 之间产生 off-by-one），故错误字符串应完全相等。
    #[tokio::test]
    async fn test_acquire_context_bail_output_identical_across_exhaustion_branches() {
        let cooldown = std::time::Duration::from_secs(3600);

        // total-exhausted 分支：单凭据。
        let mut total_cred = KiroCredentials::default();
        total_cred.access_token = Some("t1".to_string());
        total_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let total_mgr =
            MultiTokenManager::new(Config::default(), vec![total_cred], None, None, false).unwrap();
        total_mgr.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(cooldown),
        );
        let total_err = total_mgr.acquire_context().await.err().unwrap().to_string();

        // enabled-exhausted 分支：长冷却凭据（id=1）+ 一个被 quota 禁用的凭据（id=2）。
        let mut cooling_cred = KiroCredentials::default();
        cooling_cred.access_token = Some("t1".to_string());
        cooling_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let mut disabled_cred = KiroCredentials::default();
        disabled_cred.access_token = Some("t2".to_string());
        disabled_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        let enabled_mgr = MultiTokenManager::new(
            Config::default(),
            vec![cooling_cred, disabled_cred],
            None,
            None,
            false,
        )
        .unwrap();
        assert!(enabled_mgr.report_quota_exhausted(2));
        enabled_mgr.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(cooldown),
        );
        let enabled_err = enabled_mgr
            .acquire_context()
            .await
            .err()
            .unwrap()
            .to_string();

        assert_eq!(
            total_err, enabled_err,
            "两个穷尽分支在相同输入下应产出完全一致的 bail 错误"
        );
        assert!(total_err.contains("所有凭据均处于冷却/速率限制"));
        assert!(total_err.contains("来自凭据 #1"));
    }

    /// COR-001 hardening：累计短睡预算检查改为无条件后，“混合故障轮次”（一个短冷却凭据 +
    /// 一个 token 刷新失败凭据）的累计等待也会被 wait_budget 截断，不再无限短睡到禁用收敛。
    ///
    /// 关键约束：因本轮并非全部因冷却（混杂刷新失败），all_due_to_cooling=false，
    /// 故截断时 bail 的 **分类** 必须保持为常规 fallthrough 错误，而不是 429 冷却文案
    /// （否则会吞掉真实的 token 刷新失败语义）。
    ///
    /// 设计：短冷却凭据冷却 1.5s（< 2s long-wait 阈值，但 > 测试预算 300ms），刷新失败凭据
    /// 缺失 access_token/refresh_token（validate_refresh_token 同步快速失败，仅失败 1 次远
    /// 低于 MAX_FAILURES=3，不会被禁用）。第一轮决策时 1500ms > 300ms 预算即截断退出。
    #[tokio::test]
    async fn test_acquire_context_mixed_round_capped_by_budget_without_429_classification() {
        let config = Config::default();

        let mut cooling_cred = KiroCredentials::default();
        cooling_cred.access_token = Some("t1".to_string());
        cooling_cred.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cooling_cred.priority = 0;

        // 缺失 access_token / refresh_token / kiro_api_key —— try_ensure_token 会同步失败。
        let mut refresh_fail_cred = KiroCredentials::default();
        refresh_fail_cred.access_token = None;
        refresh_fail_cred.refresh_token = None;
        refresh_fail_cred.expires_at = None;
        refresh_fail_cred.priority = 0;

        let manager = MultiTokenManager::new(
            config,
            vec![cooling_cred, refresh_fail_cred],
            None,
            None,
            false,
        )
        .unwrap();

        // 1.5s 冷却：低于 2s long-wait 阈值（不触发 429 快返回），但高于 300ms 测试预算。
        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_millis(1500)),
        );

        let started = std::time::Instant::now();
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(2), manager.acquire_context())
                .await;
        let elapsed = started.elapsed();

        // 必须 bail（被预算截断），而不是无限短睡到超时。
        let err = result
            .expect("混合故障累计等待应被预算截断 bail，而非挂起到超时")
            .err()
            .expect("混合故障场景不应成功获取 context");

        // 应在远早于 1.5s 冷却结束前返回（预算截断生效，未真正睡满）。
        assert!(
            elapsed < std::time::Duration::from_millis(800),
            "应在预算耗尽后快速 bail，实际耗时: {:?}",
            elapsed
        );
        // 分类保持：混合故障必须走常规 fallthrough 文案，绝不是 429 冷却文案。
        assert!(
            !err.to_string().contains("所有凭据均处于冷却/速率限制"),
            "混合故障被预算截断时不应分类为 429 冷却：{}",
            err
        );
        assert!(
            err.to_string().contains("无法获取有效 Token"),
            "应为常规 fallthrough 错误：{}",
            err
        );
    }

    /// 混合故障场景：一个凭据长冷却，一个凭据 token 刷新失败（access_token/refresh_token 均缺失）。
    /// 期望：不应快速返回 429（会错误吞掉真实的 token 刷新失败语义），应走常规 sleep 路径。
    /// 用 tokio::time::timeout 做短超时，避免测试卡在长 sleep 循环里。
    #[tokio::test]
    async fn test_acquire_context_does_not_bail_429_on_mixed_failures() {
        let config = Config::default();

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred1.priority = 0;

        // 无 access_token / refresh_token / kiro_api_key —— try_ensure_token 会失败
        let mut cred2 = KiroCredentials::default();
        cred2.access_token = None;
        cred2.refresh_token = None;
        cred2.expires_at = None;
        cred2.priority = 0;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // cred1 长冷却（超过 2s 阈值），cred2 不设冷却但 token 刷新会失败
        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_secs(10)),
        );

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            manager.acquire_context(),
        )
        .await;

        match result {
            Err(_timeout) => {
                // 超时说明进入了 sleep 循环——正是期望的行为（未提前 bail 429）。
            }
            Ok(Ok(_)) => panic!("混合故障场景不应成功获取 context"),
            Ok(Err(err)) => {
                assert!(
                    !err.to_string().contains("所有凭据均处于冷却/速率限制"),
                    "混合故障场景不应 bail 429：{}",
                    err
                );
            }
        }
    }

    #[tokio::test]
    async fn test_multi_token_manager_acquire_context_for_user_keeps_affinity_when_bound_credential_rate_limited()
     {
        let mut config = Config::default();
        config.credential_rpm = Some(60_000);

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred1.priority = 0;

        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred2.priority = 0;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        let first = manager
            .acquire_context_for_user(Some("user-a"))
            .await
            .unwrap();
        assert_eq!(first.id, 1);

        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_millis(200)),
        );

        let diverted = manager
            .acquire_context_for_user(Some("user-a"))
            .await
            .unwrap();
        assert_eq!(diverted.id, 2);

        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        let while_cooling = manager
            .acquire_context_for_user(Some("user-a"))
            .await
            .unwrap();
        assert_eq!(while_cooling.id, 2);

        tokio::time::sleep(std::time::Duration::from_millis(220)).await;
        let rebound = manager
            .acquire_context_for_user(Some("user-a"))
            .await
            .unwrap();
        assert_eq!(rebound.id, 1);
    }

    /// 亲和性命中派发也必须记录 usage：否则被绑定凭据的流量不计入 recent_usage，
    /// LB 的 min-usage 偏好会低估其真实负载，持续向其分派新用户。
    #[tokio::test]
    async fn test_affinity_hit_dispatch_records_usage() {
        // 高 rpm 把请求间隔压到 ~1ms，配合 acquire 之间的短睡，
        // 保证后续请求走 affinity 短路而非被间隔检查分流。
        let mut config = Config::default();
        config.credential_rpm = Some(60_000);

        let mut cred1 = KiroCredentials::default();
        cred1.access_token = Some("t1".to_string());
        cred1.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred1.priority = 0;

        let mut cred2 = KiroCredentials::default();
        cred2.access_token = Some("t2".to_string());
        cred2.expires_at = Some((Utc::now() + Duration::hours(1)).to_rfc3339());
        cred2.priority = 0;

        let manager =
            MultiTokenManager::new(config, vec![cred1, cred2], None, None, false).unwrap();

        // 首次：走 LB 主路径选号并建立绑定，派发记账 1 次
        let first = manager
            .acquire_context_for_user(Some("user-a"))
            .await
            .unwrap();
        let bound_id = first.id;
        assert_eq!(manager.recent_usage_of(bound_id), 1);

        // 后两次：affinity 命中短路派发，同样必须各记账 1 次
        for expected in 2..=3u32 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            let ctx = manager
                .acquire_context_for_user(Some("user-a"))
                .await
                .unwrap();
            assert_eq!(ctx.id, bound_id, "亲和性绑定应持续命中同一凭据");
            assert_eq!(
                manager.recent_usage_of(bound_id),
                expected,
                "亲和性命中派发未记录 usage"
            );
        }
    }

    // ============ 凭据级 Region 优先级测试 ============

    /// 辅助函数：获取 OIDC 刷新使用的 region（用于测试）
    fn get_oidc_region_for_credential<'a>(
        credentials: &'a KiroCredentials,
        config: &'a Config,
    ) -> &'a str {
        credentials.region.as_ref().unwrap_or(&config.region)
    }

    #[test]
    fn test_build_idc_refresh_user_agents_uses_config_versions() {
        let mut config = Config::default();
        config.system_version = "darwin#25.4.0".to_string();
        config.node_version = "22.22.0".to_string();

        let (amz_user_agent, user_agent) = build_idc_refresh_user_agents(&config);

        assert_eq!(amz_user_agent, "aws-sdk-js/3.980.0 KiroIDE");
        assert!(user_agent.contains("os/darwin#25.4.0"));
        assert!(user_agent.contains("md/nodejs#22.22.0"));
        assert!(user_agent.contains("api/sso-oidc#3.980.0"));
    }

    #[test]
    fn test_build_usage_limit_user_agents_uses_config_versions() {
        let mut config = Config::default();
        config.kiro_version = "0.11.107".to_string();
        config.system_version = "win32#10.0.22631".to_string();
        config.node_version = "22.22.0".to_string();
        let credentials = KiroCredentials::default();
        let endpoint = IdeEndpoint::new();
        let ctx = RequestContext {
            credentials: &credentials,
            token: "test_token",
            machine_id: "machine123",
            config: &config,
        };

        let usage = endpoint.usage_request_parts(&ctx).unwrap();
        let amz_user_agent = usage
            .headers
            .iter()
            .find(|(name, _)| *name == "x-amz-user-agent")
            .map(|(_, value)| value.clone())
            .unwrap();
        let user_agent = usage
            .headers
            .iter()
            .find(|(name, _)| *name == "user-agent")
            .map(|(_, value)| value.clone())
            .unwrap();

        assert_eq!(
            amz_user_agent,
            "aws-sdk-js/1.0.0 KiroIDE-0.11.107-machine123"
        );
        assert!(user_agent.contains("os/win32#10.0.22631"));
        assert!(user_agent.contains("md/nodejs#22.22.0"));
        assert!(user_agent.contains("KiroIDE-0.11.107-machine123"));
    }

    #[test]
    fn test_credential_region_priority_uses_credential_region() {
        // 凭据配置了 region 时，应使用凭据的 region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("eu-west-1".to_string());

        let region = get_oidc_region_for_credential(&credentials, &config);
        assert_eq!(region, "eu-west-1");
    }

    #[test]
    fn test_credential_region_priority_fallback_to_config() {
        // 凭据未配置 region 时，应回退到 config.region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let credentials = KiroCredentials::default();
        assert!(credentials.region.is_none());

        let region = get_oidc_region_for_credential(&credentials, &config);
        assert_eq!(region, "us-west-2");
    }

    #[test]
    fn test_multiple_credentials_use_respective_regions() {
        // 多凭据场景下，不同凭据使用各自的 region
        let mut config = Config::default();
        config.region = "ap-northeast-1".to_string();

        let mut cred1 = KiroCredentials::default();
        cred1.region = Some("us-east-1".to_string());

        let mut cred2 = KiroCredentials::default();
        cred2.region = Some("eu-west-1".to_string());

        let cred3 = KiroCredentials::default(); // 无 region，使用 config

        assert_eq!(get_oidc_region_for_credential(&cred1, &config), "us-east-1");
        assert_eq!(get_oidc_region_for_credential(&cred2, &config), "eu-west-1");
        assert_eq!(
            get_oidc_region_for_credential(&cred3, &config),
            "ap-northeast-1"
        );
    }

    #[test]
    fn test_idc_oidc_endpoint_uses_credential_region() {
        // 验证 IdC OIDC endpoint URL 使用凭据 region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("eu-central-1".to_string());

        let region = get_oidc_region_for_credential(&credentials, &config);
        let refresh_url = format!("https://oidc.{}.amazonaws.com/token", region);

        assert_eq!(refresh_url, "https://oidc.eu-central-1.amazonaws.com/token");
    }

    #[test]
    fn test_social_refresh_endpoint_uses_credential_region() {
        // 验证 Social refresh endpoint URL 使用凭据 region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("ap-southeast-1".to_string());

        let region = get_oidc_region_for_credential(&credentials, &config);
        let refresh_url = format!("https://prod.{}.auth.desktop.kiro.dev/refreshToken", region);

        assert_eq!(
            refresh_url,
            "https://prod.ap-southeast-1.auth.desktop.kiro.dev/refreshToken"
        );
    }

    /// Round 7 corrected: this test used to assert `api_host` used config.region
    /// even when credentials had its own region — but it only did a local
    /// `format!()`, never invoked `effective_api_region` / `host()`. The actual
    /// code uses **credentials.region first, falling back to config**. This
    /// test now exercises the real `effective_api_region` to lock in that
    /// behavior.
    #[test]
    fn test_api_call_uses_credentials_region_when_set() {
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("eu-west-1".to_string());

        // The real production behavior: credentials.region wins.
        let api_region = credentials.effective_api_region(&config);
        assert_eq!(
            api_region, "eu-west-1",
            "credentials.region must take precedence over config.region"
        );

        let api_host = format!("q.{}.amazonaws.com", api_region);
        assert_eq!(api_host, "q.eu-west-1.amazonaws.com");
    }

    /// Mirror: when credentials.region is None, falls back to config.region.
    #[test]
    fn test_api_call_falls_back_to_config_region() {
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let credentials = KiroCredentials::default(); // region: None

        let api_region = credentials.effective_api_region(&config);
        assert_eq!(api_region, "us-west-2");
    }

    #[test]
    fn test_credential_region_empty_string_fallback_to_config() {
        // 空字符串 region 应回退到 config.region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("".to_string());

        let region = credentials
            .region
            .as_ref()
            .filter(|r| !r.trim().is_empty())
            .unwrap_or(&config.region);
        // 空字符串应回退到 config.region
        assert_eq!(region, "us-west-2");
    }

    #[test]
    fn test_credential_region_whitespace_fallback_to_config() {
        // 纯空白字符 region 应回退到 config.region
        let mut config = Config::default();
        config.region = "us-west-2".to_string();

        let mut credentials = KiroCredentials::default();
        credentials.region = Some("   ".to_string());

        let region = credentials
            .region
            .as_ref()
            .filter(|r| !r.trim().is_empty())
            .unwrap_or(&config.region);
        assert_eq!(region, "us-west-2");
    }

    // ============ Keepalive 保活探测测试 ============

    /// 构造单凭据 manager（keepalive 测试公共脚手架）
    fn new_keepalive_manager(config: Config) -> MultiTokenManager {
        MultiTokenManager::new(config, vec![KiroCredentials::default()], None, None, false).unwrap()
    }

    #[test]
    fn test_keepalive_due_idle_over_threshold() {
        let manager = new_keepalive_manager(Config::default());
        // 空闲 3h > 默认阈值 2h
        manager.set_last_used_at_for_test(1, Some((Utc::now() - Duration::hours(3)).to_rfc3339()));
        assert!(manager.keepalive_due(1));
    }

    #[test]
    fn test_keepalive_due_not_idle_under_threshold() {
        let manager = new_keepalive_manager(Config::default());
        // 刚使用过，未达阈值
        manager.set_last_used_at_for_test(1, Some(Utc::now().to_rfc3339()));
        assert!(!manager.keepalive_due(1));
    }

    #[test]
    fn test_keepalive_due_none_last_used_falls_back_to_start_time() {
        let manager = new_keepalive_manager(Config::default());
        // last_used_at 保持 None：空闲起点为 manager 构造时间，刚构造未超阈值
        assert!(!manager.keepalive_due(1));
        // 回拨构造时间 3h，应判定空闲超阈值
        manager.set_keepalive_started_at_for_test(Utc::now() - Duration::hours(3));
        assert!(manager.keepalive_due(1));
    }

    #[test]
    fn test_keepalive_due_skips_cooling_credential() {
        let manager = new_keepalive_manager(Config::default());
        // 注意顺序：set_credential_cooldown_with_duration 会覆写 last_used_at=now，
        // 必须先设冷却、再回拨 last_used_at，才能证明 false 来自冷却闸而非 idle 闸
        manager.set_credential_cooldown_with_duration(
            1,
            CooldownReason::RateLimitExceeded,
            Some(std::time::Duration::from_secs(3600)),
        );
        manager.set_last_used_at_for_test(1, Some((Utc::now() - Duration::hours(3)).to_rfc3339()));
        assert!(!manager.keepalive_due(1));
    }

    #[test]
    fn test_keepalive_probe_throttle() {
        let manager = new_keepalive_manager(Config::default());
        manager.set_last_used_at_for_test(1, Some((Utc::now() - Duration::hours(3)).to_rfc3339()));
        assert!(manager.keepalive_due(1));

        // 探测后进入节流期，不再重复探测
        manager.mark_keepalive_probed(1);
        assert!(!manager.keepalive_due(1));

        // 回拨节流表超过一个阈值（7300s > 7200s），应重新到期。
        // Instant 零点为系统启动时刻，开机不足 7300s 时 checked_sub 下溢返回 None
        // 无法构造过去时刻——此环境限制下跳过该断言（前两个断言已覆盖节流生效路径）
        if let Some(past) =
            std::time::Instant::now().checked_sub(std::time::Duration::from_secs(7300))
        {
            manager.set_keepalive_probed_for_test(1, past);
            assert!(manager.keepalive_due(1));
        }
    }

    #[test]
    fn test_keepalive_threshold_clamped_to_min() {
        // 显式正值低于下限时钳制到 MIN_KEEPALIVE_IDLE_THRESHOLD_SECS（600），防误配每 tick 全量探测
        let mut config = Config::default();
        config.keepalive_idle_threshold_seconds = Some(1);
        assert_eq!(
            config.effective_keepalive_idle_threshold(),
            Some(crate::model::config::MIN_KEEPALIVE_IDLE_THRESHOLD_SECS)
        );
        // 高于下限的显式值原样放行
        config.keepalive_idle_threshold_seconds = Some(3600);
        assert_eq!(config.effective_keepalive_idle_threshold(), Some(3600));
    }

    #[test]
    fn test_keepalive_disabled_when_threshold_zero() {
        let mut config = Config::default();
        config.keepalive_idle_threshold_seconds = Some(0);
        let manager = new_keepalive_manager(config);
        // 阈值 0 = 禁用：即便空闲超阈值也不探测（行为与改动前完全一致）
        manager.set_last_used_at_for_test(1, Some((Utc::now() - Duration::hours(3)).to_rfc3339()));
        assert!(!manager.keepalive_due(1));
    }

    #[test]
    fn test_keepalive_probe_does_not_touch_last_used_at() {
        let manager = new_keepalive_manager(Config::default());
        let ts0 = (Utc::now() - Duration::hours(3)).to_rfc3339();
        manager.set_last_used_at_for_test(1, Some(ts0.clone()));

        let before = manager.snapshot();
        let entry_before = before.entries.iter().find(|e| e.id == 1).unwrap();
        let success_before = entry_before.success_count;

        manager.mark_keepalive_probed(1);

        // 探测打点绝不污染业务统计：last_used_at 与 success_count 均不变
        let after = manager.snapshot();
        let entry_after = after.entries.iter().find(|e| e.id == 1).unwrap();
        assert_eq!(entry_after.last_used_at.as_deref(), Some(ts0.as_str()));
        assert_eq!(entry_after.success_count, success_before);
    }

    #[test]
    fn test_update_default_endpoint() {
        let mut config = Config::default();
        config.default_endpoint = "ide".to_string();

        let credentials = KiroCredentials::default();
        let manager = MultiTokenManager::new(config, vec![credentials], None, None, false).unwrap();

        assert_eq!(manager.config().default_endpoint, "ide");

        manager.update_default_endpoint("cli".to_string());
        assert_eq!(manager.config().default_endpoint, "cli");

        manager.update_default_endpoint("ide".to_string());
        assert_eq!(manager.config().default_endpoint, "ide");
    }
}
