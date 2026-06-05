//! 余额缓存管理


pub(super) struct CachedBalance {
    pub(super) remaining: f64,
    pub(super) cached_at: std::time::Instant,
    /// 是否已初始化（区分"未获取过余额"和"余额为零"）
    pub(super) initialized: bool,
    /// 最近一段时间的使用次数（用于判断高频/低频）
    pub(super) recent_usage: u32,
    /// 上次重置使用计数的时间
    pub(super) usage_reset_at: std::time::Instant,
}

/// 高频渠道 TTL（10 分钟）
pub(super) const BALANCE_TTL_HIGH_FREQ_SECS: u64 = 600;
/// 低频渠道 TTL（30 分钟）
pub(super) const BALANCE_TTL_LOW_FREQ_SECS: u64 = 1800;
/// 低余额渠道 TTL（24 小时）
pub(super) const BALANCE_TTL_LOW_BALANCE_SECS: u64 = 86400;
/// 高频判定阈值（10分钟内使用超过此次数视为高频）
pub(super) const HIGH_FREQ_THRESHOLD: u32 = 20;
/// 使用计数重置周期（10 分钟）
pub(super) const USAGE_COUNT_RESET_SECS: u64 = 600;
/// 低余额阈值（用于动态 TTL 判断）
pub(super) const LOW_BALANCE_THRESHOLD: f64 = 1.0;
