//! 精细化速率限制系统
//!
//! 实现每日请求限制、请求间隔控制、指数退避等策略，
//! 模拟人类使用模式，降低被检测风险。
//! 参考 CLIProxyAPIPlus 的实现。

use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// 默认每日最大请求数
pub const DEFAULT_DAILY_MAX_REQUESTS: u32 = 500;

/// 默认最小请求间隔（毫秒）
const DEFAULT_MIN_INTERVAL_MS: u64 = 1000;

/// 默认最大请求间隔（毫秒）
const DEFAULT_MAX_INTERVAL_MS: u64 = 2000;

/// 默认抖动百分比
const DEFAULT_JITTER_PERCENT: f64 = 0.3;

/// 速率限制配置
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// 每日最大请求数
    ///
    /// `None` 表示禁用每日请求上限。
    pub daily_max_requests: Option<u32>,

    /// 最小请求间隔（毫秒）
    pub min_interval_ms: u64,

    /// 最大请求间隔（毫秒）
    pub max_interval_ms: u64,

    /// 抖动百分比（0.0 - 1.0）
    pub jitter_percent: f64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            daily_max_requests: Some(DEFAULT_DAILY_MAX_REQUESTS),
            min_interval_ms: DEFAULT_MIN_INTERVAL_MS,
            max_interval_ms: DEFAULT_MAX_INTERVAL_MS,
            jitter_percent: DEFAULT_JITTER_PERCENT,
        }
    }
}

/// 凭据速率状态
#[derive(Debug, Clone)]
struct CredentialRateState {
    /// 今日请求计数
    daily_count: u32,

    /// 计数重置时间
    count_reset_at: Instant,

    /// 上次请求时间
    last_request_at: Option<Instant>,
}

impl Default for CredentialRateState {
    fn default() -> Self {
        Self {
            daily_count: 0,
            count_reset_at: Instant::now() + Duration::from_secs(86400),
            last_request_at: None,
        }
    }
}

/// 速率限制器
///
/// 管理所有凭据的速率限制状态
pub struct RateLimiter {
    config: RwLock<RateLimitConfig>,
    states: Mutex<HashMap<u64, CredentialRateState>>,
}

impl RateLimiter {
    /// 创建新的速率限制器
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config: RwLock::new(config),
            states: Mutex::new(HashMap::new()),
        }
    }

    /// 使用默认配置创建速率限制器
    #[cfg(test)]
    pub fn with_defaults() -> Self {
        Self::new(RateLimitConfig::default())
    }

    /// 热更新速率限制配置
    pub fn update_config(&self, new_config: RateLimitConfig) {
        *self.config.write() = new_config;
    }

    /// 检查凭据是否可以发送请求
    ///
    /// 返回 `Ok(())` 表示可以发送，`Err(Duration)` 表示需要等待的时间
    pub fn check_rate_limit(&self, credential_id: u64) -> Result<(), Duration> {
        let config = self.config.read().clone();
        let mut states = self.states.lock();
        let state = states.entry(credential_id).or_default();
        let now = Instant::now();

        // 检查是否需要重置每日计数
        if now >= state.count_reset_at {
            state.daily_count = 0;
            state.count_reset_at = now + Duration::from_secs(86400);
        }

        // 检查每日限制
        if let Some(daily_max_requests) = config.daily_max_requests
            && state.daily_count >= daily_max_requests
        {
            let wait_time = state.count_reset_at.saturating_duration_since(now);
            return Err(wait_time);
        }

        // 检查请求间隔
        if let Some(last_request) = state.last_request_at {
            let min_interval = Self::calculate_interval_with_config(&config);
            let elapsed = now.saturating_duration_since(last_request);
            if elapsed < min_interval {
                return Err(min_interval - elapsed);
            }
        }

        Ok(())
    }

    /// 尝试获取一次“发送许可”（原子检查 + 占位）
    ///
    /// `check_rate_limit()` 仅做检查，不会更新状态，无法在并发场景下避免“同时放行”。
    /// 本方法在同一把锁内完成检查与 `last_request_at` 更新，用于：
    /// - 限制单个凭据的请求频率（近似 RPM/最小间隔）
    /// - 在并发请求下将流量自然分流到其他可用凭据
    ///
    /// 返回 `Ok(())` 表示已占用一个发送窗口；`Err(Duration)` 表示需要等待的时间。
    pub fn try_acquire(&self, credential_id: u64) -> Result<(), Duration> {
        let config = self.config.read().clone();
        let min_interval = Self::calculate_interval_with_config(&config);

        let mut states = self.states.lock();
        let state = states.entry(credential_id).or_default();
        let now = Instant::now();

        // 检查是否需要重置每日计数
        if now >= state.count_reset_at {
            state.daily_count = 0;
            state.count_reset_at = now + Duration::from_secs(86400);
        }

        // 检查每日限制
        if let Some(daily_max_requests) = config.daily_max_requests
            && state.daily_count >= daily_max_requests
        {
            let wait_time = state.count_reset_at.saturating_duration_since(now);
            return Err(wait_time);
        }

        // 检查请求间隔
        if let Some(last_request) = state.last_request_at {
            let elapsed = now.saturating_duration_since(last_request);
            if elapsed < min_interval {
                return Err(min_interval - elapsed);
            }
        }

        // 占位：更新上次请求时间，避免并发下同一凭据被同时放行
        state.last_request_at = Some(now);
        Ok(())
    }

    /// 记录请求成功
    pub fn record_success(&self, credential_id: u64) {
        let mut states = self.states.lock();
        let state = states.entry(credential_id).or_default();

        state.daily_count = state.daily_count.saturating_add(1);
        state.last_request_at = Some(Instant::now());
    }

    /// 重置凭据的速率限制状态
    pub fn reset(&self, credential_id: u64) {
        let mut states = self.states.lock();
        states.remove(&credential_id);
    }

    /// 计算请求间隔（带抖动）
    fn calculate_interval_with_config(config: &RateLimitConfig) -> Duration {
        let base = (config.min_interval_ms + config.max_interval_ms) / 2;
        let jitter_range = (base as f64 * config.jitter_percent) as u64;
        let jitter = if jitter_range > 0 {
            fastrand::u64(0..=jitter_range * 2) as i64 - jitter_range as i64
        } else {
            0
        };
        let interval = (base as i64 + jitter)
            .max(config.min_interval_ms as i64)
            .min(config.max_interval_ms as i64) as u64;
        Duration::from_millis(interval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_new() {
        let limiter = RateLimiter::with_defaults();
        assert!(limiter.check_rate_limit(1).is_ok());
    }

    #[test]
    fn test_rate_limiter_daily_limit() {
        let config = RateLimitConfig {
            daily_max_requests: Some(2),
            min_interval_ms: 0,
            max_interval_ms: 0,
            ..Default::default()
        };
        let limiter = RateLimiter::new(config);

        // 前两次请求应该成功
        assert!(limiter.check_rate_limit(1).is_ok());
        limiter.record_success(1);
        assert!(limiter.check_rate_limit(1).is_ok());
        limiter.record_success(1);

        // 第三次应该被限制
        assert!(limiter.check_rate_limit(1).is_err());
    }

    #[test]
    fn test_rate_limiter_daily_limit_can_be_disabled() {
        let config = RateLimitConfig {
            daily_max_requests: None,
            min_interval_ms: 0,
            max_interval_ms: 0,
            ..Default::default()
        };
        let limiter = RateLimiter::new(config);

        for _ in 0..DEFAULT_DAILY_MAX_REQUESTS + 10 {
            assert!(limiter.check_rate_limit(1).is_ok());
            limiter.record_success(1);
        }
    }

    #[test]
    fn test_rate_limiter_reset() {
        let config = RateLimitConfig {
            daily_max_requests: Some(1),
            min_interval_ms: 0,
            max_interval_ms: 0,
            ..Default::default()
        };
        let limiter = RateLimiter::new(config);

        // 打满每日上限后被限制
        limiter.record_success(1);
        assert!(limiter.check_rate_limit(1).is_err());

        // reset 后恢复可用
        limiter.reset(1);
        assert!(limiter.check_rate_limit(1).is_ok());
    }
}
