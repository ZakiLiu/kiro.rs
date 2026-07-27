use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TlsBackend {
    #[default]
    Rustls,
    NativeTls,
}

/// Claude Code 内置工具兼容模式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ToolCompatibilityMode {
    /// 将 Claude Code 内置工具名、schema 与参数适配为 Kiro 内置工具。
    #[default]
    ClaudeCode,
    /// 保留原有透传行为，用于兼容与排障。
    Raw,
}

/// 自定义模型定义。
///
/// 用户在 `config.json` 的 `customModels` 数组里声明客户端模型别名到 Kiro 后端
/// 模型 ID 的映射及元数据。运行期由 [`crate::model::custom_models`] 全局注册表按
/// `id`（大小写不敏感）精确匹配，优先于内置的模糊映射逻辑——既能新增模型，也能
/// 覆盖内置模型的映射。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomModel {
    /// 客户端请求时使用的模型名（别名）。匹配大小写不敏感。
    pub id: String,

    /// 映射到的 Kiro 后端模型 ID（实际下发给上游）。
    pub backend_id: String,

    /// `/v1/models` 展示名（可选，缺省用 `id`）。
    #[serde(default)]
    pub display_name: Option<String>,

    /// 上下文窗口大小（可选，缺省 200000）。
    #[serde(default)]
    pub context_window: Option<i32>,

    /// 单次响应最大 token 数，用于 `/v1/models` 展示（可选，缺省 64000）。
    #[serde(default)]
    pub max_tokens: Option<i32>,

    /// 是否支持原生 reasoning / `output_config`（可选，缺省 false）。
    /// 命中的自定义模型置 true 时，会按 backend_id 放行 `additionalModelRequestFields`。
    #[serde(default)]
    pub supports_reasoning: Option<bool>,

    /// `/v1/models` 的 `owned_by` 字段（可选，缺省 "custom"）。
    #[serde(default)]
    pub owned_by: Option<String>,
}

/// Prompt Preset 预设
///
/// 可通过 `x-preset-id` 请求头选择，将 system_prompt 前置注入到请求中。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    /// 预设唯一 ID（用于 x-preset-id 匹配）
    pub id: String,
    /// 预设名称（用于展示）
    pub name: String,
    /// 要前置注入的 system prompt
    pub system_prompt: String,
    /// 是否启用
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// KNA 应用配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_host")]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default = "default_region")]
    pub region: String,

    /// API Region（用于 API 请求），未配置时回退到 region
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,

    #[serde(default = "default_kiro_version")]
    pub kiro_version: String,

    #[serde(default)]
    pub machine_id: Option<String>,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default = "default_system_version")]
    pub system_version: String,

    #[serde(default = "default_node_version")]
    pub node_version: String,

    #[serde(default = "default_tls_backend")]
    pub tls_backend: TlsBackend,

    /// 外部 count_tokens API 地址（可选）
    #[serde(default)]
    pub count_tokens_api_url: Option<String>,

    /// count_tokens API 密钥（可选）
    #[serde(default)]
    pub count_tokens_api_key: Option<String>,

    /// count_tokens API 认证类型（可选，"x-api-key" 或 "bearer"，默认 "x-api-key"）
    #[serde(default = "default_count_tokens_auth_type")]
    pub count_tokens_auth_type: String,

    /// HTTP 代理地址（可选）
    /// 支持格式: http://host:port, https://host:port, socks5://host:port
    #[serde(default)]
    pub proxy_url: Option<String>,

    /// 代理认证用户名（可选）
    #[serde(default)]
    pub proxy_username: Option<String>,

    /// 代理认证密码（可选）
    #[serde(default)]
    pub proxy_password: Option<String>,

    /// Admin API 密钥（可选，启用 Admin API 功能）
    #[serde(default)]
    pub admin_api_key: Option<String>,

    /// Social OAuth 公网回调基址（可选）。
    /// 配置后（如 `https://example.com/api/admin/auth/callback`）：OAuth `redirect_uri`
    /// 改用此地址，浏览器授权后落到 `{callbackBaseUrl}/oauth/callback`，
    /// 由本服务公网路由自动接收，适合远程部署。
    /// 未配置时 Admin UI 会按当前访问 origin 自动派生；非 UI 客户端仍可回落本地端口模式。
    #[serde(default)]
    pub callback_base_url: Option<String>,

    /// 单个凭据的目标请求速率（RPM，每分钟请求数）
    ///
    /// 用于凭据级节流/分流：当某个凭据短时间内请求过密时，优先将流量分配到其他可用凭据，
    /// 从而减少上游 429 的概率。
    ///
    /// - `None`: 使用内置默认节流策略
    /// - `0`: 禁用本地凭据级 RPM 节流；若未单独设置 `credentialDailyMaxRequests`，
    ///   同时禁用默认每日请求上限
    /// - `>0`: 将最小/最大请求间隔固定为 `60_000 / rpm` 毫秒
    #[serde(default)]
    pub credential_rpm: Option<u32>,

    /// 单个凭据的每日最大请求数
    ///
    /// - `None`: 使用内置默认值；但当 `credentialRpm` 显式为 `0` 时，默认不启用每日上限
    /// - `0`: 禁用每日请求上限
    /// - `>0`: 使用指定每日请求上限
    #[serde(default)]
    pub credential_daily_max_requests: Option<u32>,

    /// 凭据保活探测的空闲阈值（秒）
    ///
    /// 凭据空闲超过该阈值后，周期性 balance 刷新循环会强制探测一次（keepalive），
    /// 避免低余额凭据因 24h 余额缓存 TTL 长期无上游调用、token 失效却无人发现。
    ///
    /// - `None`: 使用内置默认阈值（7200 秒 = 2 小时）
    /// - `0`: 禁用保活探测
    /// - `>0`: 显式空闲阈值（秒），下限钳制 600（防误配造成每 tick 全量探测）
    #[serde(default)]
    pub keepalive_idle_threshold_seconds: Option<u64>,

    /// 输入压缩配置
    #[serde(default)]
    pub compression: CompressionConfig,

    /// Prompt Cache TTL（秒），默认 300 秒。对外协议仍按最多 1 小时上报。
    #[serde(default = "default_prompt_cache_ttl_seconds")]
    pub prompt_cache_ttl_seconds: u64,

    /// 是否启用本地 Prompt Cache usage 记账，默认 true
    #[serde(default = "default_true")]
    pub prompt_cache_accounting_enabled: bool,

    /// 默认端点名称（凭据未显式指定 endpoint 时使用）
    #[serde(default = "default_endpoint")]
    pub default_endpoint: String,

    /// 是否启用请求指标收集，默认 true
    #[serde(default = "default_true")]
    pub metrics_enabled: bool,

    /// 指标环形缓冲区大小，默认 10000
    #[serde(default = "default_metrics_ring_buffer_size")]
    pub metrics_ring_buffer_size: usize,

    /// 是否启用跨请求缓存，默认 true
    #[serde(default = "default_true")]
    pub cross_request_cache_enabled: bool,

    /// 跨请求缓存最大条目数，默认 1000
    #[serde(default = "default_cross_request_cache_max_entries")]
    pub cross_request_cache_max_entries: usize,

    /// Prompt 预设列表
    #[serde(default)]
    pub presets: Vec<Preset>,

    /// 自定义模型定义列表（客户端别名 → Kiro 后端模型 ID）
    ///
    /// 优先级最高，先于内置模糊映射查询；未命中时回退到原有映射逻辑。
    #[serde(default)]
    pub custom_models: Vec<CustomModel>,

    // ── 运维管理（从 OTHER 移植） ──
    /// 负载均衡模式："priority"（默认，按优先级）或 "balanced"（均衡分配）
    #[serde(default = "default_load_balancing_mode")]
    pub load_balancing_mode: String,

    /// 是否启用请求追踪（SQLite），默认 false
    #[serde(default)]
    pub trace_enabled: bool,

    /// 追踪日志保留天数，默认 7
    #[serde(default = "default_trace_retention_days")]
    pub trace_retention_days: u32,

    /// 用量日志保留天数，默认 31
    #[serde(default = "default_usage_log_retention_days")]
    pub usage_log_retention_days: u32,

    /// 账户级限流时是否故障转移到下一个凭据，默认 false
    #[serde(default)]
    pub account_throttle_failover: bool,

    /// 账户级限流冷却秒数，默认 300
    #[serde(default = "default_account_throttle_cooldown_secs")]
    pub account_throttle_cooldown_secs: u64,

    /// 非 streaming 场景是否提取 thinking 内容，默认 false
    #[serde(default)]
    pub extract_thinking: bool,

    /// Claude Code 内置工具兼容模式，默认 `claude-code`。
    #[serde(default = "default_tool_compatibility_mode")]
    pub tool_compatibility_mode: ToolCompatibilityMode,

    // ── 系统提示词控制 ──
    /// 是否剥离客户端 system prompt 中的安全限制，默认 false
    #[serde(default)]
    pub strip_system_restrictions: bool,

    /// 系统提示词注入总开关，默认 false
    #[serde(default)]
    pub system_prompt_enabled: bool,

    /// 启用的内置 preset ID 列表（如 ["override", "pentest"]）
    #[serde(default)]
    pub enabled_presets: Vec<String>,

    /// 自定义补充系统提示词
    #[serde(default)]
    pub system_prompt: Option<String>,

    /// 系统提示词注入位置，默认 Append
    #[serde(default)]
    pub system_prompt_position: SystemPromptPosition,

    // ── 在线更新 ──
    /// 上一次更新前运行的版本号（带 v 前缀）
    #[serde(default)]
    pub update_previous_version: Option<String>,

    /// GitHub Personal Access Token（提升 API 限流配额）
    #[serde(default)]
    pub github_token: Option<String>,

    /// 上一次成功更新的时间（RFC 3339）
    #[serde(default)]
    pub update_last_applied_at: Option<String>,

    /// 是否开启无人值守自动更新
    #[serde(default)]
    pub update_auto_apply: bool,

    /// 自动更新触发时间（HH:MM，本地 24 小时制），默认 03:00
    #[serde(default = "default_auto_apply_time")]
    pub update_auto_apply_time: String,

    /// 配置文件路径（运行时元数据，不写入 JSON）
    #[serde(skip)]
    config_path: Option<PathBuf>,
}

/// 系统提示词注入位置
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SystemPromptPosition {
    Prepend,
    #[default]
    Append,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_region() -> String {
    "us-east-1".to_string()
}

fn default_kiro_version() -> String {
    "0.11.107".to_string()
}

fn default_system_version() -> String {
    const SYSTEM_VERSIONS: &[&str] = &["darwin#24.6.0", "win32#10.0.22631"];
    SYSTEM_VERSIONS[fastrand::usize(..SYSTEM_VERSIONS.len())].to_string()
}

fn default_node_version() -> String {
    "22.22.0".to_string()
}

fn default_count_tokens_auth_type() -> String {
    "x-api-key".to_string()
}

fn default_endpoint() -> String {
    "ide".to_string()
}

fn default_load_balancing_mode() -> String {
    "priority".to_string()
}

fn default_trace_retention_days() -> u32 {
    7
}

fn default_usage_log_retention_days() -> u32 {
    31
}

fn default_account_throttle_cooldown_secs() -> u64 {
    300
}

fn default_auto_apply_time() -> String {
    "03:00".to_string()
}

fn default_tool_compatibility_mode() -> ToolCompatibilityMode {
    ToolCompatibilityMode::ClaudeCode
}

pub const PROMPT_CACHE_TTL_5M_SECONDS: u64 = 300;
pub const PROMPT_CACHE_TTL_1H_SECONDS: u64 = 3600;
pub const PROMPT_CACHE_TTL_2H_SECONDS: u64 = 7200;
pub const PROMPT_CACHE_TTL_5H_SECONDS: u64 = 18000;

pub fn is_supported_prompt_cache_ttl_seconds(ttl_seconds: u64) -> bool {
    matches!(
        ttl_seconds,
        PROMPT_CACHE_TTL_5M_SECONDS
            | PROMPT_CACHE_TTL_1H_SECONDS
            | PROMPT_CACHE_TTL_2H_SECONDS
            | PROMPT_CACHE_TTL_5H_SECONDS
    )
}

fn default_prompt_cache_ttl_seconds() -> u64 {
    PROMPT_CACHE_TTL_5M_SECONDS
}

fn default_metrics_ring_buffer_size() -> usize {
    10_000
}

fn default_cross_request_cache_max_entries() -> usize {
    1_000
}

fn default_tls_backend() -> TlsBackend {
    TlsBackend::Rustls
}

/// keepalive 空闲阈值默认值（秒）：2 小时
pub const DEFAULT_KEEPALIVE_IDLE_THRESHOLD_SECS: u64 = 7200;

/// keepalive 空闲阈值下限（秒）：10 分钟（与 balance 刷新周期同量级），
/// 显式正值低于此值时钳制，防误配造成每 tick 全量探测（参照 balance interval .max(60) 防御先例）
pub const MIN_KEEPALIVE_IDLE_THRESHOLD_SECS: u64 = 600;

fn default_true() -> bool {
    true
}

fn default_thinking_strategy() -> String {
    "discard".to_string()
}

fn default_8000() -> usize {
    8000
}

fn default_80() -> usize {
    80
}

fn default_40() -> usize {
    40
}

fn default_6000() -> usize {
    6000
}

fn default_4000() -> usize {
    4000
}

fn default_80_turns() -> usize {
    80
}

fn default_400k() -> usize {
    400_000
}

fn default_image_max_long_edge() -> u32 {
    4000
}

fn default_image_max_pixels_single() -> u32 {
    4_000_000
}

fn default_image_max_pixels_multi() -> u32 {
    4_000_000
}

fn default_image_multi_threshold() -> usize {
    20
}

fn default_max_request_body_bytes() -> usize {
    // 上游对请求体大小存在硬性限制（实测约 5MiB 左右会触发 400），
    // 这里默认设置为 4.5MiB 留出安全余量。
    4_718_592
}

fn default_max_input_tokens() -> usize {
    200_000
}

/// 输入压缩配置
///
/// 控制请求体在协议转换后、发送到上游前的多层压缩策略。
/// 所有阈值均可通过配置文件调整，默认开启。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressionConfig {
    /// 总开关，默认 true
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 空白压缩（连续空行、行尾空格），默认 true
    #[serde(default = "default_true")]
    pub whitespace_compression: bool,
    /// thinking 块处理策略: "discard" | "truncate" | "keep"
    #[serde(default = "default_thinking_strategy")]
    pub thinking_strategy: String,
    /// tool_result 截断阈值（字符数），默认 8000
    #[serde(default = "default_8000")]
    pub tool_result_max_chars: usize,
    /// 智能截断保留头部行数，默认 80
    #[serde(default = "default_80")]
    pub tool_result_head_lines: usize,
    /// 智能截断保留尾部行数，默认 40
    #[serde(default = "default_40")]
    pub tool_result_tail_lines: usize,
    /// tool_use input 截断阈值（字符数），默认 6000
    #[serde(default = "default_6000")]
    pub tool_use_input_max_chars: usize,
    /// 工具描述截断阈值（字符数），覆盖原 10000 硬编码，默认 4000
    #[serde(default = "default_4000")]
    pub tool_description_max_chars: usize,
    /// 历史最大轮数，默认 80（0=不限）
    #[serde(default = "default_80_turns")]
    pub max_history_turns: usize,
    /// 历史最大字符数，默认 400000（0=不限）
    #[serde(default = "default_400k")]
    pub max_history_chars: usize,
    /// 图片长边最大像素，默认 4000（Anthropic 硬限制 8000，留安全余量；窄长图受益于更大长边）
    #[serde(default = "default_image_max_long_edge")]
    pub image_max_long_edge: u32,
    /// 单张图片最大总像素，默认 4_000_000（2000×2000，与多图限制一致）
    #[serde(default = "default_image_max_pixels_single")]
    pub image_max_pixels_single: u32,
    /// 多图模式下单张图片最大总像素，默认 4_000_000（2000×2000）
    #[serde(default = "default_image_max_pixels_multi")]
    pub image_max_pixels_multi: u32,
    /// 触发多图限制的图片数量阈值，默认 20
    #[serde(default = "default_image_multi_threshold")]
    pub image_multi_threshold: usize,
    /// 请求体最大字节数，超过则直接拒绝（0 = 不限制）
    #[serde(default = "default_max_request_body_bytes")]
    pub max_request_body_bytes: usize,
    /// 输入 token 上限（超过此值触发自适应压缩），默认 200000（0 = 不限制）
    #[serde(default = "default_max_input_tokens")]
    pub max_input_tokens: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            whitespace_compression: true,
            thinking_strategy: default_thinking_strategy(),
            tool_result_max_chars: default_8000(),
            tool_result_head_lines: default_80(),
            tool_result_tail_lines: default_40(),
            tool_use_input_max_chars: default_6000(),
            tool_description_max_chars: default_4000(),
            max_history_turns: default_80_turns(),
            max_history_chars: default_400k(),
            image_max_long_edge: default_image_max_long_edge(),
            image_max_pixels_single: default_image_max_pixels_single(),
            image_max_pixels_multi: default_image_max_pixels_multi(),
            image_multi_threshold: default_image_multi_threshold(),
            max_request_body_bytes: default_max_request_body_bytes(),
            max_input_tokens: default_max_input_tokens(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            region: default_region(),
            api_region: None,
            kiro_version: default_kiro_version(),
            machine_id: None,
            api_key: None,
            system_version: default_system_version(),
            node_version: default_node_version(),
            tls_backend: default_tls_backend(),
            count_tokens_api_url: None,
            count_tokens_api_key: None,
            count_tokens_auth_type: default_count_tokens_auth_type(),
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            admin_api_key: None,
            callback_base_url: None,
            credential_rpm: None,
            credential_daily_max_requests: None,
            keepalive_idle_threshold_seconds: None,
            compression: CompressionConfig::default(),
            prompt_cache_ttl_seconds: default_prompt_cache_ttl_seconds(),
            prompt_cache_accounting_enabled: default_true(),
            default_endpoint: default_endpoint(),
            metrics_enabled: default_true(),
            metrics_ring_buffer_size: default_metrics_ring_buffer_size(),
            cross_request_cache_enabled: default_true(),
            cross_request_cache_max_entries: default_cross_request_cache_max_entries(),
            presets: Vec::new(),
            custom_models: Vec::new(),
            load_balancing_mode: default_load_balancing_mode(),
            trace_enabled: false,
            trace_retention_days: default_trace_retention_days(),
            usage_log_retention_days: default_usage_log_retention_days(),
            account_throttle_failover: false,
            account_throttle_cooldown_secs: default_account_throttle_cooldown_secs(),
            extract_thinking: false,
            tool_compatibility_mode: default_tool_compatibility_mode(),
            strip_system_restrictions: false,
            system_prompt_enabled: false,
            enabled_presets: Vec::new(),
            system_prompt: None,
            system_prompt_position: SystemPromptPosition::Append,
            update_previous_version: None,
            github_token: None,
            update_last_applied_at: None,
            update_auto_apply: false,
            update_auto_apply_time: default_auto_apply_time(),
            config_path: None,
        }
    }
}

impl Config {
    /// 获取默认配置文件路径
    pub fn default_config_path() -> &'static str {
        "config.json"
    }

    /// 获取有效的 API Region（用于 API 请求）
    /// 优先使用 api_region，未配置时回退到 region
    #[allow(dead_code)]
    pub fn effective_api_region(&self) -> &str {
        self.api_region.as_deref().unwrap_or(&self.region)
    }

    /// 计算有效的 keepalive 空闲阈值（秒）
    ///
    /// - `None` → 默认 `DEFAULT_KEEPALIVE_IDLE_THRESHOLD_SECS`（7200）
    /// - `Some(0)` → `None`（禁用保活探测）
    /// - `Some(n)` → `Some(n.max(600))`——下限钳制 `MIN_KEEPALIVE_IDLE_THRESHOLD_SECS`，
    ///   防止极小阈值令 idle 判定与节流同时失效，造成每个 balance tick 全量探测上游
    pub fn effective_keepalive_idle_threshold(&self) -> Option<u64> {
        match self.keepalive_idle_threshold_seconds {
            None => Some(DEFAULT_KEEPALIVE_IDLE_THRESHOLD_SECS),
            Some(0) => None,
            Some(n) => Some(n.max(MIN_KEEPALIVE_IDLE_THRESHOLD_SECS)),
        }
    }

    /// 从文件加载配置
    pub fn load<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if !path.exists() {
            // 配置文件不存在，返回默认配置
            return Ok(Self {
                config_path: Some(path.to_path_buf()),
                ..Default::default()
            });
        }

        let content = fs::read_to_string(path)?;
        let mut config: Config = serde_json::from_str(&content)?;
        config.config_path = Some(path.to_path_buf());
        Ok(config)
    }

    /// 获取配置文件路径（如果有）
    #[allow(dead_code)]
    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    /// 将当前配置写回原始配置文件
    #[allow(dead_code)]
    pub fn save(&self) -> anyhow::Result<()> {
        let path = self
            .config_path
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("配置文件路径未知，无法保存配置"))?;

        let content = serde_json::to_string_pretty(self).context("序列化配置失败")?;
        fs::write(path, content)
            .with_context(|| format!("写入配置文件失败: {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_defaults_enable_prompt_cache_accounting() {
        let config = Config::default();
        assert!(config.prompt_cache_accounting_enabled);
    }

    #[test]
    fn test_tool_compatibility_mode_defaults_to_claude_code() {
        let config: Config = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(
            config.tool_compatibility_mode,
            ToolCompatibilityMode::ClaudeCode
        );
    }

    #[test]
    fn test_tool_compatibility_mode_deserializes_raw() {
        let config: Config = serde_json::from_str(r#"{"toolCompatibilityMode":"raw"}"#).unwrap();
        assert_eq!(config.tool_compatibility_mode, ToolCompatibilityMode::Raw);
    }

    #[test]
    fn test_config_deserializes_prompt_cache_accounting_false() {
        let config: Config = serde_json::from_str(r#"{"promptCacheAccountingEnabled":false}"#)
            .expect("config should deserialize");
        assert!(!config.prompt_cache_accounting_enabled);
    }

    #[test]
    fn test_config_deserializes_presets_array() {
        let config: Config = serde_json::from_str(
            r#"{"presets":[{"id":"test","name":"Test","systemPrompt":"You are helpful","enabled":true}]}"#,
        )
        .expect("config with presets should deserialize");
        assert_eq!(config.presets.len(), 1);
        assert_eq!(config.presets[0].id, "test");
        assert_eq!(config.presets[0].name, "Test");
        assert_eq!(config.presets[0].system_prompt, "You are helpful");
        assert!(config.presets[0].enabled);
    }

    #[test]
    fn test_config_defaults_presets_empty() {
        let config: Config =
            serde_json::from_str(r#"{}"#).expect("config without presets should deserialize");
        assert!(config.presets.is_empty());
    }

    #[test]
    fn test_preset_enabled_defaults_true() {
        let preset: Preset = serde_json::from_str(r#"{"id":"p1","name":"P1","systemPrompt":"hi"}"#)
            .expect("preset should deserialize without enabled field");
        assert!(preset.enabled);
    }

    #[test]
    fn test_compression_config_defaults_max_input_tokens() {
        let config = CompressionConfig::default();

        assert_eq!(config.max_input_tokens, 200_000);
    }

    #[test]
    fn test_config_deserializes_compression_max_input_tokens() {
        let config: Config = serde_json::from_str(r#"{"compression":{"maxInputTokens":123456}}"#)
            .expect("config with maxInputTokens should deserialize");

        assert_eq!(config.compression.max_input_tokens, 123_456);
        assert_eq!(
            config.compression.max_request_body_bytes,
            default_max_request_body_bytes()
        );
    }
}
