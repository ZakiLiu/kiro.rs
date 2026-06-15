//! Admin API 类型定义

use crate::admin::proxy_pool::ProxyHealth;
use serde::{Deserialize, Serialize};

// ============ 凭据状态 ============

/// 所有凭据状态响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialsStatusResponse {
    /// 凭据总数
    pub total: usize,
    /// 未禁用凭据数量（兼容旧字段，不代表可立即承接请求）
    pub available: usize,
    /// 可立即承接请求的凭据数量
    pub ready: usize,
    /// 当前处于 cooldown 的未禁用凭据数量
    pub cooling: usize,
    /// 各凭据状态列表
    pub credentials: Vec<CredentialStatusItem>,
}

/// 单个凭据的状态信息
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStatusItem {
    /// 凭据唯一 ID
    pub id: u64,
    /// 优先级（数字越小优先级越高）
    pub priority: u32,
    /// 是否被禁用
    pub disabled: bool,
    /// 连续失败次数
    pub failure_count: u32,
    /// Token 刷新连续失败次数
    pub refresh_failure_count: u32,
    /// 禁用原因
    pub disabled_reason: Option<String>,
    /// 是否可立即承接请求（未禁用、未冷却、未触发本地速率限制）
    pub ready: bool,
    /// 冷却原因
    pub cooldown_reason: Option<String>,
    /// 冷却剩余秒数
    pub cooldown_remaining_secs: Option<u64>,
    /// 是否被本地速率限制挡住
    pub rate_limited: bool,
    /// 本地速率限制剩余秒数
    pub rate_limit_remaining_secs: Option<u64>,
    /// Token 过期时间（RFC3339 格式）
    pub expires_at: Option<String>,
    /// 认证方式
    pub auth_method: Option<String>,
    /// 是否有 Profile ARN
    pub has_profile_arn: bool,
    /// refreshToken 的 SHA-256 哈希（用于前端重复检测）
    pub refresh_token_hash: Option<String>,
    /// 用户邮箱（用于前端显示）
    pub email: Option<String>,
    /// 已持久化的订阅等级（页面刷新后可直接展示）
    pub subscription_title: Option<String>,
    /// API 调用成功次数
    pub success_count: u64,
    /// 最后一次 API 调用时间（RFC3339 格式）
    pub last_used_at: Option<String>,
    /// 凭据级 Region（用于 Token 刷新）
    pub region: Option<String>,
    /// 凭据级 API Region（单独覆盖 API 请求）
    pub api_region: Option<String>,
    /// 凭据显式配置的 endpoint（None 表示回退到 defaultEndpoint）
    pub endpoint: Option<String>,
    /// 最终生效的 endpoint 名称
    pub effective_endpoint: String,
}

// ============ 操作请求 ============

/// 启用/禁用凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDisabledRequest {
    /// 是否禁用
    pub disabled: bool,
}

/// 修改优先级请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPriorityRequest {
    /// 新优先级值
    pub priority: u32,
}

/// 修改 Region 请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetRegionRequest {
    /// 凭据级 Region（用于 Token 刷新），空字符串表示清除
    pub region: Option<String>,
    /// 凭据级 API Region（单独覆盖 API 请求），空字符串表示清除
    pub api_region: Option<String>,
}

/// 修改 endpoint 请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetEndpointRequest {
    /// endpoint 名称，空字符串或 null 表示回退到 defaultEndpoint
    pub endpoint: Option<String>,
}

/// 添加凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialRequest {
    /// 刷新令牌（OAuth 凭据必填，API Key 凭据不需要）
    pub refresh_token: Option<String>,

    /// Kiro API Key（API Key 凭据必填）
    pub kiro_api_key: Option<String>,

    /// 认证方式（可选，默认 social）
    #[serde(default = "default_auth_method")]
    pub auth_method: String,

    /// OIDC Client ID（IdC 认证需要）
    pub client_id: Option<String>,

    /// OIDC Client Secret（IdC 认证需要）
    pub client_secret: Option<String>,

    /// 优先级（可选，默认 0）
    #[serde(default)]
    pub priority: u32,

    /// 凭据级 Region 配置（用于 Token 刷新）
    /// 未配置时回退到 config.json 的全局 region
    pub region: Option<String>,

    /// 凭据级 API Region（用于 API 调用）
    pub api_region: Option<String>,

    /// 凭据级 Machine ID（可选，64 位字符串）
    /// 未配置时回退到 config.json 的 machineId
    pub machine_id: Option<String>,

    /// 凭据级 endpoint（未配置时回退到 config.defaultEndpoint；当前已注册端点由服务端校验）
    pub endpoint: Option<String>,

    /// 用户邮箱（可选，用于前端显示）
    pub email: Option<String>,

    /// 凭据级代理 URL
    pub proxy_url: Option<String>,

    /// 凭据级代理用户名
    pub proxy_username: Option<String>,

    /// 凭据级代理密码
    pub proxy_password: Option<String>,
}

fn default_auth_method() -> String {
    "social".to_string()
}

/// 添加凭据成功响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCredentialResponse {
    pub success: bool,
    pub message: String,
    /// 新添加的凭据 ID
    pub credential_id: u64,
    /// 用户邮箱（如果获取成功）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

// ============ 余额查询 ============

/// 余额查询响应
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BalanceResponse {
    /// 凭据 ID
    pub id: u64,
    /// 订阅类型
    pub subscription_title: Option<String>,
    /// 当前使用量
    pub current_usage: f64,
    /// 使用限额
    pub usage_limit: f64,
    /// 剩余额度
    pub remaining: f64,
    /// 使用百分比
    pub usage_percentage: f64,
    /// 下次重置时间（Unix 时间戳）
    pub next_reset_at: Option<f64>,
}

/// 缓存余额信息
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedBalanceItem {
    /// 凭据 ID
    pub id: u64,
    /// 缓存的剩余额度
    pub remaining: f64,
    /// 使用限额
    pub usage_limit: f64,
    /// 使用百分比
    pub usage_percentage: f64,
    /// 订阅类型
    pub subscription_title: Option<String>,
    /// 缓存时间（Unix 毫秒时间戳）
    pub cached_at: u64,
    /// 缓存存活时间（秒），缓存过期时间 = cached_at + ttl_secs * 1000
    pub ttl_secs: u64,
}

/// 所有凭据的缓存余额响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedBalancesResponse {
    /// 各凭据的缓存余额列表
    pub balances: Vec<CachedBalanceItem>,
}

// ============ 负载均衡配置 ============

// ============ 全局代理配置 ============

/// 全局代理配置响应
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfigResponse {
    pub proxy_url: Option<String>,
    pub has_credentials: bool,
}

/// 更新全局代理配置请求
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProxyConfigRequest {
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
}

// ============ 通用响应 ============

/// 操作成功响应
#[derive(Debug, Serialize)]
pub struct SuccessResponse {
    pub success: bool,
    pub message: String,
}

impl SuccessResponse {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
        }
    }
}

// ============ 批量导入 token.json ============

/// 官方 token.json 格式（用于解析导入）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenJsonItem {
    pub provider: Option<String>,
    pub refresh_token: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub auth_method: Option<String>,
    #[serde(default)]
    pub priority: u32,
    pub region: Option<String>,
    pub api_region: Option<String>,
    pub machine_id: Option<String>,
}

/// 批量导入请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTokenJsonRequest {
    #[serde(default = "default_dry_run")]
    pub dry_run: bool,
    pub items: ImportItems,
}

fn default_dry_run() -> bool {
    true
}

/// 导入项（支持单个或数组）
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ImportItems {
    Single(TokenJsonItem),
    Multiple(Vec<TokenJsonItem>),
}

impl ImportItems {
    pub fn into_vec(self) -> Vec<TokenJsonItem> {
        match self {
            ImportItems::Single(item) => vec![item],
            ImportItems::Multiple(items) => items,
        }
    }
}

/// 批量导入响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportTokenJsonResponse {
    pub summary: ImportSummary,
    pub items: Vec<ImportItemResult>,
}

/// 导入汇总
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    pub parsed: usize,
    pub added: usize,
    pub skipped: usize,
    pub invalid: usize,
}

/// 单项导入结果
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemResult {
    pub index: usize,
    pub fingerprint: String,
    pub action: ImportAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<u64>,
}

/// 导入动作
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImportAction {
    Added,
    Skipped,
    Invalid,
}

/// 错误响应
#[derive(Debug, Serialize)]
pub struct AdminErrorResponse {
    pub error: AdminError,
}

#[derive(Debug, Serialize)]
pub struct AdminError {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl AdminErrorResponse {
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: AdminError {
                error_type: error_type.into(),
                message: message.into(),
            },
        }
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new("invalid_request", message)
    }

    pub fn authentication_error() -> Self {
        Self::new("authentication_error", "Invalid or missing admin API key")
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new("not_found", message)
    }

    pub fn api_error(message: impl Into<String>) -> Self {
        Self::new("api_error", message)
    }

    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new("internal_error", message)
    }
}

// ============ 指标聚合 ============

/// 指标概览响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSummaryResponse {
    /// 总请求数
    pub total_requests: usize,
    /// 成功请求数
    pub successful: usize,
    /// 失败请求数
    pub failed: usize,
    /// 平均延迟（毫秒）
    pub avg_latency_ms: f64,
    /// 总输入 token 数
    pub total_input_tokens: i64,
    /// 总输出 token 数
    pub total_output_tokens: i64,
    /// 统计窗口内的事件总数
    pub window_size: usize,
}

/// 按模型聚合的指标
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMetrics {
    /// 模型名称
    pub model: String,
    /// 请求数
    pub request_count: usize,
    /// 平均延迟（毫秒）
    pub avg_latency_ms: f64,
    /// 总输入 token 数
    pub total_input_tokens: i64,
    /// 总输出 token 数
    pub total_output_tokens: i64,
}

/// 按凭据聚合的指标
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialMetrics {
    /// 凭据 ID
    pub credential_id: u64,
    /// 请求数
    pub request_count: usize,
    /// 成功请求数
    pub success_count: usize,
    /// 失败请求数
    pub failure_count: usize,
    /// 平均延迟（毫秒）
    pub avg_latency_ms: f64,
}

// ============ 全局配置 ============

/// 全局配置响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalConfigResponse {
    /// AWS Region
    pub region: String,
    /// 单凭据目标请求速率（RPM），None 表示默认策略，0 表示关闭本地 RPM 节流
    pub credential_rpm: Option<u32>,
    /// 单凭据每日最大请求数，None 表示使用默认策略，0 表示关闭每日上限
    pub credential_daily_max_requests: Option<u32>,
    /// Prompt Cache TTL（秒）
    pub prompt_cache_ttl_seconds: u64,
    /// 是否启用本地 Prompt Cache usage 记账
    pub prompt_cache_accounting_enabled: bool,
    /// 默认端点名称（凭据未显式指定 endpoint 时使用）
    pub default_endpoint: String,
    /// 压缩配置
    pub compression: CompressionConfigResponse,
}

/// 压缩配置响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressionConfigResponse {
    pub enabled: bool,
    pub whitespace_compression: bool,
    pub thinking_strategy: String,
    pub tool_result_max_chars: usize,
    pub tool_result_head_lines: usize,
    pub tool_result_tail_lines: usize,
    pub tool_use_input_max_chars: usize,
    pub tool_description_max_chars: usize,
    pub max_history_turns: usize,
    pub max_history_chars: usize,
    pub max_request_body_bytes: usize,
}

/// 区分「字段缺失」与「显式 null」的双层 Option 反序列化：
/// 缺失 → `None`（不更新），null → `Some(None)`（恢复默认），值 → `Some(Some(v))`。
/// 裸 `Option<Option<T>>` 会把 null 吃成外层 `None`，导致“清空恢复默认”静默失败。
fn deserialize_double_option<'de, T, D>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(de).map(Some)
}

/// 更新全局配置请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGlobalConfigRequest {
    /// AWS Region（可选）
    pub region: Option<String>,
    /// 单凭据目标请求速率（RPM；缺失不更新，null 恢复默认，0 关闭本地节流）
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub credential_rpm: Option<Option<u32>>,
    /// 单凭据每日最大请求数（缺失不更新，null 恢复默认，0 关闭每日上限）
    #[serde(default, deserialize_with = "deserialize_double_option")]
    pub credential_daily_max_requests: Option<Option<u32>>,
    /// Prompt Cache TTL（秒，可选，仅支持 300 或 3600）
    pub prompt_cache_ttl_seconds: Option<u64>,
    /// 是否启用本地 Prompt Cache usage 记账（可选）
    pub prompt_cache_accounting_enabled: Option<bool>,
    /// 默认端点名称（可选）
    pub default_endpoint: Option<String>,
    /// 压缩配置（可选）
    pub compression: Option<UpdateCompressionConfigRequest>,
}

// ============ Prompt 预设 ============

/// 创建预设请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreatePresetRequest {
    /// 预设唯一 ID
    pub id: String,
    /// 预设名称
    pub name: String,
    /// 要前置注入的 system prompt
    pub system_prompt: String,
    /// 是否启用（默认 true）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// 更新预设请求（所有字段可选）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePresetRequest {
    /// 预设名称
    pub name: Option<String>,
    /// 要前置注入的 system prompt
    pub system_prompt: Option<String>,
    /// 是否启用
    pub enabled: Option<bool>,
}

/// 更新压缩配置请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCompressionConfigRequest {
    pub enabled: Option<bool>,
    pub whitespace_compression: Option<bool>,
    pub thinking_strategy: Option<String>,
    pub tool_result_max_chars: Option<usize>,
    pub tool_result_head_lines: Option<usize>,
    pub tool_result_tail_lines: Option<usize>,
    pub tool_use_input_max_chars: Option<usize>,
    pub tool_description_max_chars: Option<usize>,
    pub max_history_turns: Option<usize>,
    pub max_history_chars: Option<usize>,
    pub max_request_body_bytes: Option<usize>,
}

// ============ 负载均衡模式 ============

/// 负载均衡模式响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadBalancingModeResponse {
    /// 当前模式（"priority" 或 "balanced"）
    pub mode: String,
}

/// 设置负载均衡模式请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLoadBalancingModeRequest {
    /// 模式（"priority" 或 "balanced"）
    pub mode: String,
}

// ============ 账号级风控 ============

/// 账号级风控故障转移配置响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountThrottleConfigResponse {
    /// 是否启用账号级 429 故障转移
    pub failover: bool,
    /// 冷却时长（秒）
    pub cooldown_secs: u64,
}

/// 更新账号级风控故障转移配置
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetAccountThrottleConfigRequest {
    /// 是否启用故障转移；缺省表示不修改
    #[serde(default)]
    pub failover: Option<bool>,
    /// 冷却时长（秒）；缺省表示不修改，1..=86400
    #[serde(default)]
    pub cooldown_secs: Option<u64>,
}

// ============ 日志治理 ============

/// 日志治理配置响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogGovernanceConfigResponse {
    /// 是否启用请求链路追踪写入
    pub trace_enabled: bool,
    /// trace 记录保留天数
    pub trace_retention_days: u32,
    /// 用量日志保留天数
    pub usage_log_retention_days: u32,
}

/// 更新日志治理配置（字段缺省表示不修改）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetLogGovernanceConfigRequest {
    #[serde(default)]
    pub trace_enabled: Option<bool>,
    /// trace 保留天数，1..=365
    #[serde(default)]
    pub trace_retention_days: Option<u32>,
    /// 用量日志保留天数，1..=365
    #[serde(default)]
    pub usage_log_retention_days: Option<u32>,
}

// ============ 代理池 ============

/// 代理池条目
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolEntry {
    /// 唯一 ID（自增）
    pub id: u64,
    /// 代理 URL（如 socks5://user:pass@host:port）
    pub url: String,
    /// 备注标签（可选）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// 是否启用
    pub enabled: bool,
    /// 使用此代理的凭据数量
    pub credential_count: u32,
    /// 健康状态
    pub health: ProxyHealth,
    /// 最近一次成功探测的延迟（毫秒）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    /// 最近一次探测时间（RFC3339）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    /// 连续探测失败计数
    pub consecutive_failures: u32,
    /// 是否由健康检查自动禁用
    pub auto_disabled: bool,
}

/// 代理池列表响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolResponse {
    pub total: usize,
    pub proxies: Vec<ProxyPoolEntry>,
}

/// 单个代理健康检查响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCheckResponse {
    pub id: u64,
    pub health: ProxyHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<String>,
    pub enabled: bool,
    pub auto_disabled: bool,
}

/// 全量健康检查响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCheckAllResponse {
    pub healthy: usize,
    pub unhealthy: usize,
    pub auto_disabled: usize,
}

/// 轮询批量分配请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignRoundRobinRequest {
    /// 目标凭据 ID 列表；为空或缺省表示对全部凭据分配
    #[serde(default)]
    pub credential_ids: Option<Vec<u64>>,
}

/// 轮询批量分配响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignRoundRobinResponse {
    /// 成功分配的凭据数
    pub assigned: usize,
    /// 参与轮询的可用代理数
    pub proxy_count: usize,
}

/// 添加代理请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddProxyRequest {
    pub url: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// 批量导入代理请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchAddProxyRequest {
    /// 代理 URL 列表（每行一个）
    pub urls: Vec<String>,
}

/// 分配代理给凭据请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignProxyRequest {
    /// 代理池中的代理 ID；null 表示清除代理
    #[serde(default)]
    pub proxy_id: Option<u64>,
}

// ============ 全局代理配置（新） ============

/// 全局代理配置响应（仅 URL）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalProxyResponse {
    /// 当前全局代理 URL（null 表示未配置）
    pub proxy_url: Option<String>,
}

/// 设置全局代理请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetGlobalProxyRequest {
    /// 代理 URL，null 表示清除全局代理
    pub proxy_url: Option<String>,
}

// ============ 登录API密钥修改 ============

/// 修改登录API密钥（管理面板登录用 adminApiKey）请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAdminKeyRequest {
    /// 新的登录API密钥
    pub new_key: String,
}

// ============ 凭据更新 ============

/// 更新凭据请求（仅可编辑字段，None 表示不修改，Some("") 表示清除）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCredentialRequest {
    /// 用户邮箱（用于前端显示）
    pub email: Option<String>,
    /// 凭据级代理 URL（空字符串表示清除）
    pub proxy_url: Option<String>,
    /// 凭据级代理认证用户名
    pub proxy_username: Option<String>,
    /// 凭据级代理认证密码
    pub proxy_password: Option<String>,
    /// 账号所属分组（None 表示不修改，Some 表示整体替换）
    #[serde(default)]
    pub groups: Option<Vec<String>>,
    /// 账号来源渠道（None 表示不修改，空串表示清除）
    #[serde(default)]
    pub source_channel: Option<String>,
}

/// 更新 refreshToken 请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRefreshTokenRequest {
    /// 新的刷新令牌
    pub refresh_token: String,
    /// 可选：同时更新 accessToken
    #[serde(default)]
    pub access_token: Option<String>,
    /// 可选：同时更新 expiresAt
    #[serde(default)]
    pub expires_at: Option<String>,
}

// ============ 客户端 API Key 分发 ============

/// 客户端 Key 列表项（脱敏展示）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientKeyItem {
    pub id: u64,
    /// 脱敏后的 Key 展示（如 csk_abcd...mnop）
    pub masked_key: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub disabled: bool,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    pub total_calls: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// 是否系统密钥（config.json apiKey 导入，不可删除）
    #[serde(default)]
    pub is_system: bool,
}

/// 客户端 Key 列表响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientKeysResponse {
    pub total: usize,
    pub keys: Vec<ClientKeyItem>,
}

/// 创建客户端 Key 请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateClientKeyRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
}

/// 创建客户端 Key 响应（明文 Key 仅在此处返回一次）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateClientKeyResponse {
    pub id: u64,
    pub key: String,
    pub name: String,
    pub created_at: String,
}

/// 更新客户端 Key 元数据
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateClientKeyRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
}

// ============ IdC 设备授权登录 ============

/// 发起 IdC 设备授权请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartIdcLoginRequest {
    pub region: String,
    #[serde(default)]
    pub start_url: Option<String>,
    #[serde(default)]
    pub priority: u32,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub proxy_url: Option<String>,
}

// ============ Social 登录（Portal PKCE OAuth） ============

/// 发起 Social 登录请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSocialLoginRequest {
    /// 优先级（默认 0）
    #[serde(default)]
    pub priority: u32,
    /// 用户邮箱（可选）
    #[serde(default)]
    pub email: Option<String>,
    /// 代理 URL（可选）
    #[serde(default)]
    pub proxy_url: Option<String>,
    /// Kiro auth endpoint（留空用默认）
    #[serde(default)]
    pub auth_endpoint: Option<String>,
}

/// 手动完成 Social 登录请求（远程访问场景：从浏览器地址栏复制回调 URL）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteSocialLoginRequest {
    /// OAuth 授权码（从回调 URL 的 code 参数提取）
    pub code: String,
    /// OAuth state（从回调 URL 的 state 参数提取，用于 CSRF 校验）
    pub state: String,
    /// 登录选项（从回调 URL 的 login_option 参数提取，可为空）
    #[serde(default)]
    pub login_option: String,
    /// 回调 URL 的路径（如 /oauth/callback）
    #[serde(default = "default_oauth_path")]
    pub path: String,
}

fn default_oauth_path() -> String {
    "/oauth/callback".to_string()
}

// ============ 账号分组（独立实体）============

/// 单条分组（列表项）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupItem {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    /// 引用计数：有多少个凭据带这个分组
    pub credential_count: usize,
    /// 引用计数：有多少把客户端 Key 绑定这个分组
    pub client_key_count: usize,
}

/// 分组列表响应
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupsResponse {
    pub total: usize,
    pub groups: Vec<GroupItem>,
}

/// 创建分组请求
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// 更新分组请求（改名 / 改备注；两者都可选）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGroupRequest {
    /// 新名字；不传或与原名一致则不改名
    #[serde(default)]
    pub new_name: Option<String>,
    /// 新备注；传空字符串清除备注；不传字段则保留
    #[serde(default)]
    pub description: Option<String>,
}

/// 删除分组的可选查询参数
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGroupQuery {
    /// 强制删除：即使仍有引用也删；同时级联清理凭据 / Key 的引用
    #[serde(default)]
    pub force: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 限流字段必须三态可分：缺失=不更新，null=恢复默认，值=设定。
    /// 回归保护：裸 Option<Option<T>> 会把 null 吃成外层 None，
    /// 导致前端“清空恢复默认”静默失败。
    #[test]
    fn test_update_global_config_rate_limit_fields_distinguish_null_from_missing() {
        let missing: UpdateGlobalConfigRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.credential_rpm, None);
        assert_eq!(missing.credential_daily_max_requests, None);

        let null_case: UpdateGlobalConfigRequest =
            serde_json::from_str(r#"{"credentialRpm": null, "credentialDailyMaxRequests": null}"#)
                .unwrap();
        assert_eq!(null_case.credential_rpm, Some(None));
        assert_eq!(null_case.credential_daily_max_requests, Some(None));

        let value_case: UpdateGlobalConfigRequest =
            serde_json::from_str(r#"{"credentialRpm": 60, "credentialDailyMaxRequests": 0}"#)
                .unwrap();
        assert_eq!(value_case.credential_rpm, Some(Some(60)));
        assert_eq!(value_case.credential_daily_max_requests, Some(Some(0)));
    }
}
