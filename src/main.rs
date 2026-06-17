mod admin;
mod admin_ui;
mod anthropic;
mod common;
mod gemini;
mod http_client;
pub mod image;
mod kiro;
pub mod metrics;
mod model;
mod openai;
#[cfg(feature = "pdf-support")]
pub mod pdf;
pub mod token;

use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use kiro::endpoint::{CliEndpoint, IdeEndpoint, KiroEndpoint};
use kiro::model::credentials::{CredentialsConfig, KiroCredentials};
use kiro::provider::KiroProvider;
use kiro::token_manager::MultiTokenManager;
use model::arg::Args;
use model::config::Config;
use parking_lot::RwLock;

#[tokio::main]
async fn main() {
    // 解析命令行参数
    let args = Args::parse();

    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // 加载配置
    let config_path = args
        .config
        .unwrap_or_else(|| Config::default_config_path().to_string());
    let config = Config::load(&config_path).unwrap_or_else(|e| {
        tracing::error!("加载配置失败: {}", e);
        std::process::exit(1);
    });
    let config = Arc::new(RwLock::new(config));

    // 加载凭证（支持单对象或数组格式）
    let credentials_path = args
        .credentials
        .unwrap_or_else(|| KiroCredentials::default_credentials_path().to_string());
    let credentials_config = CredentialsConfig::load(&credentials_path).unwrap_or_else(|e| {
        tracing::error!("加载凭证失败: {}", e);
        std::process::exit(1);
    });

    // 判断是否为多凭据格式（用于刷新后回写）
    let is_multiple_format = credentials_config.is_multiple();

    // 转换为按优先级排序的凭据列表
    let mut credentials_list = credentials_config.into_sorted_credentials();

    if let Ok(kiro_api_key) = std::env::var("KIRO_API_KEY")
        && !kiro_api_key.trim().is_empty()
    {
        tracing::info!("检测到 KIRO_API_KEY 环境变量，添加 API Key 凭据（最高优先级）");
        credentials_list.insert(
            0,
            KiroCredentials {
                kiro_api_key: Some(kiro_api_key),
                auth_method: Some("api_key".to_string()),
                priority: u32::MIN,
                runtime_only: true,
                ..Default::default()
            },
        );
    }

    tracing::info!("已加载 {} 个凭据配置", credentials_list.len());

    let mut endpoints: HashMap<String, Arc<dyn KiroEndpoint>> = HashMap::new();
    {
        let ide: Arc<dyn KiroEndpoint> = Arc::new(IdeEndpoint::new());
        endpoints.insert(ide.name().to_string(), ide);
        let cli: Arc<dyn KiroEndpoint> = Arc::new(CliEndpoint::new());
        endpoints.insert(cli.name().to_string(), cli);
    }
    let endpoint_names: Vec<String> = endpoints.keys().cloned().collect();

    let default_endpoint = config.read().default_endpoint.clone();
    if !endpoints.contains_key(&default_endpoint) {
        tracing::error!(
            "第一阶段仅支持已注册 endpoint，当前 defaultEndpoint={} 不受支持，已注册: {:?}",
            default_endpoint,
            endpoint_names
        );
        std::process::exit(1);
    }
    for cred in &credentials_list {
        let endpoint = cred.effective_endpoint_name(Some(&default_endpoint));
        if !endpoints.contains_key(endpoint) {
            tracing::error!(
                "第一阶段仅支持已注册 endpoint，凭据 id={:?} 指定了未支持 endpoint={}，已注册: {:?}",
                cred.id,
                endpoint,
                endpoint_names
            );
            std::process::exit(1);
        }
    }

    // 获取第一个凭据用于日志显示
    let first_credentials = credentials_list.first().cloned().unwrap_or_default();
    #[cfg(feature = "sensitive-logs")]
    tracing::debug!("主凭证: {:?}", first_credentials);
    #[cfg(not(feature = "sensitive-logs"))]
    tracing::debug!(
        id = ?first_credentials.id,
        priority = first_credentials.priority,
        has_profile_arn = first_credentials.profile_arn.is_some(),
        has_expires_at = first_credentials.expires_at.is_some(),
        auth_method = ?first_credentials.auth_method.as_deref(),
        "主凭证摘要"
    );

    // 获取 API Key
    let api_key = config.read().api_key.clone().unwrap_or_else(|| {
        tracing::error!("配置文件中未设置 apiKey");
        std::process::exit(1);
    });

    // 安全检查：空字符串 / 纯空白 apiKey 会让认证中间件对空 key 放行，
    // 等同于关闭代理端点认证。与 admin_api_key 的空值校验保持一致，启动即退出。
    if api_key.trim().is_empty() {
        tracing::error!("apiKey 不能为空字符串，否则代理端点将失去认证保护");
        std::process::exit(1);
    }

    // 构建代理配置
    let proxy_config = {
        let cfg = config.read();
        cfg.proxy_url.as_ref().map(|url| {
            let mut proxy = http_client::ProxyConfig::new(url);
            if let (Some(username), Some(password)) = (&cfg.proxy_username, &cfg.proxy_password) {
                proxy = proxy.with_auth(username, password);
            }
            proxy
        })
    };

    if proxy_config.is_some() {
        tracing::info!(
            "已配置 HTTP 代理: {}",
            config.read().proxy_url.as_ref().unwrap()
        );
    }

    // 创建 MultiTokenManager 和 KiroProvider
    let token_manager = MultiTokenManager::new(
        config.read().clone(),
        credentials_list,
        proxy_config.clone(),
        Some(credentials_path.clone().into()),
        is_multiple_format,
    )
    .unwrap_or_else(|e| {
        tracing::error!("创建 Token 管理器失败: {}", e);
        std::process::exit(1);
    });
    let token_manager = Arc::new(token_manager);

    // 初始化余额缓存并按余额选择初始凭据
    let init_count = token_manager.initialize_balances().await;
    if init_count == 0 && token_manager.total_count() > 0 {
        tracing::warn!("所有凭据余额初始化失败，将按优先级选择凭据");
    }

    let kiro_provider = KiroProvider::with_proxy(
        token_manager.clone(),
        proxy_config.clone(),
        endpoints,
        default_endpoint.clone(),
    );
    let kiro_provider = Arc::new(kiro_provider);

    // P0#3：启动周期性 balance 刷新（10 分钟），避免 LB 长期基于陈旧 cache 决策
    kiro_provider.start_periodic_balance_refresh(600);

    // 启动周期性凭据恢复（5 分钟），自动恢复被错误禁用的凭据
    kiro_provider.start_periodic_recovery(300);

    // 启动后台 Token 刷新，防止长时间空闲导致 Token 过期
    kiro_provider.start_background_token_refresh();

    // 初始化 count_tokens 配置
    {
        let cfg = config.read();
        token::init_config(token::CountTokensConfig {
            api_url: cfg.count_tokens_api_url.clone(),
            api_key: cfg.count_tokens_api_key.clone(),
            auth_type: cfg.count_tokens_auth_type.clone(),
            proxy: proxy_config,
            tls_backend: cfg.tls_backend,
        });
    }

    // 创建共享的压缩配置（供 Anthropic 路由和 Admin API 共用，支持热更新）
    let compression_config = Arc::new(RwLock::new(config.read().compression.clone()));
    let prompt_cache_runtime = Arc::new(RwLock::new(anthropic::PromptCacheRuntime::new(
        config.read().prompt_cache_ttl_seconds,
        config.read().prompt_cache_accounting_enabled,
    )));

    // 构建指标收集器（可通过配置禁用）
    let metrics_collector = if config.read().metrics_enabled {
        let size = config.read().metrics_ring_buffer_size;
        tracing::info!(ring_buffer_size = size, "指标收集已启用");
        Some(Arc::new(metrics::MetricsCollector::new(size)))
    } else {
        tracing::info!("指标收集已禁用");
        None
    };

    // 构建跨请求缓存（可通过配置禁用）
    let cross_request_cache = if config.read().cross_request_cache_enabled {
        let max_entries = config.read().cross_request_cache_max_entries;
        tracing::info!(max_entries, "跨请求缓存已启用");
        Some(Arc::new(
            anthropic::cross_request_cache::CrossRequestCache::new(max_entries),
        ))
    } else {
        tracing::info!("跨请求缓存已禁用");
        None
    };

    // 构建 Prompt 预设（从配置加载，共享引用供 Admin API 运行时 CRUD）
    let presets = Arc::new(RwLock::new(config.read().presets.clone()));

    // ── 运维模块初始化 ──

    // 请求追踪（SQLite）
    let trace_store = if config.read().trace_enabled {
        let db_path = std::path::Path::new(&credentials_path)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join("traces.db");
        let retention = config.read().trace_retention_days;
        match admin::trace_db::TraceStore::open(db_path.clone(), true, retention) {
            Ok(store) => {
                let store = Arc::new(store);
                tracing::info!("请求追踪已启用 (SQLite: {})", db_path.display());
                Some(store)
            }
            Err(e) => {
                tracing::warn!("请求追踪初始化失败: {}, 将禁用追踪", e);
                None
            }
        }
    } else {
        None
    };

    // 用量统计（JSONL）
    let usage_dir = std::path::Path::new(&credentials_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("usage_logs");
    let usage_recorder = Arc::new(
        admin::usage_stats::UsageRecorder::with_retention(
            usage_dir.clone(),
            config.read().usage_log_retention_days as i64,
        ),
    );
    let usage_aggregator = Arc::new(admin::usage_stats::UsageAggregator::new());
    usage_aggregator.rebuild_from_logs(&usage_dir);
    tracing::info!("用量统计已启用 (保留 {} 天)", config.read().usage_log_retention_days);

    // 启动定期清理任务（每 24 小时清理过期 usage_log 和 trace 记录）
    {
        let recorder = usage_recorder.clone();
        let ts = trace_store.clone();
        tokio::spawn(async move {
            let day = std::time::Duration::from_secs(24 * 3600);
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            loop {
                recorder.cleanup_old_logs();
                if let Some(store) = &ts {
                    store.cleanup();
                }
                tokio::time::sleep(day).await;
            }
        });
    }

    // Client Key 管理器
    let client_keys_path = std::path::Path::new(&credentials_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("client_keys.json");
    let client_key_manager = Arc::new(
        admin::client_keys::ClientKeyManager::load(&client_keys_path)
            .unwrap_or_else(|_| {
                admin::client_keys::ClientKeyManager::new()
            }),
    );

    // 分组管理器
    let groups_path = std::path::Path::new(&credentials_path)
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("groups.json");
    let group_manager = Arc::new(
        admin::groups::GroupManager::load(&groups_path)
            .unwrap_or_else(|_| {
                admin::groups::GroupManager::new()
            }),
    );
    // 从凭据和客户端 Key 中反向补注册分组（含自动分组 Free/Pro/Pro+）
    {
        let cred_groups: Vec<String> = token_manager
            .clone_all_credentials()
            .into_iter()
            .flat_map(|c| c.groups)
            .collect();
        let key_groups: Vec<String> = client_key_manager
            .used_group_names()
            .into_iter()
            .collect();
        let all_groups: Vec<String> = cred_groups.into_iter().chain(key_groups).collect();
        let added = group_manager.bootstrap_from_existing(all_groups);
        if added > 0 {
            tracing::info!("从凭据/Key 反向补注册了 {} 个分组", added);
        }
    }
    if !presets.read().is_empty() {
        tracing::info!(count = presets.read().len(), "已加载 Prompt 预设");
    }

    // 构建 Anthropic API 路由（从第一个凭据获取 profile_arn）
    let prompt_config = model::runtime::shared_from_config(&config.read());
    let mut app_state = anthropic::middleware::AppState::new(&api_key, prompt_cache_runtime.clone())
        .with_kiro_provider(kiro_provider.clone())
        .with_compression_config(compression_config.clone())
        .with_presets(presets.clone())
        .with_client_keys(client_key_manager.clone())
        .with_usage_recorder(usage_recorder.clone())
        .with_usage_aggregator(usage_aggregator.clone())
        .with_prompt_config(prompt_config);
    if let Some(arn) = &first_credentials.profile_arn {
        app_state = app_state.with_profile_arn(arn);
    }
    if let Some(m) = &metrics_collector {
        app_state = app_state.with_metrics(m.clone());
    }
    if let Some(cache) = cross_request_cache {
        app_state = app_state.with_cross_request_cache(cache);
    }
    if let Some(ts) = &trace_store {
        app_state = app_state.with_trace_store(ts.clone());
    }
    // Bootstrap 系统 Key（幂等，把 config.json apiKey 注册为 id=0 客户端 Key）
    client_key_manager.ensure_system_key(
        "System Default".to_string(),
        Some("从 config.json apiKey 自动导入".to_string()),
        api_key.clone(),
    );
    let anthropic_app = anthropic::router::create_router_from_state(app_state);

    // 构建 Admin API 路由（如果配置了非空的 admin_api_key）
    // 安全检查：空字符串被视为未配置，防止空 key 绕过认证
    let admin_key_valid = config
        .read()
        .admin_api_key
        .as_ref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    let app = {
        let cfg = config.read();
        if let Some(admin_key) = &cfg.admin_api_key {
            if admin_key.trim().is_empty() {
                tracing::warn!("admin_api_key 配置为空，Admin API 未启用");
                anthropic_app
            } else {
                let admin_service = admin::AdminService::new(
                    token_manager.clone(),
                    Some(kiro_provider.clone()),
                    config.clone(),
                    compression_config.clone(),
                    prompt_cache_runtime.clone(),
                    metrics_collector.clone(),
                    endpoint_names.clone(),
                );
                let mut admin_state =
                    admin::AdminState::new(admin_key, admin_service)
                        .with_presets(presets.clone())
                        .with_client_keys(client_key_manager.clone())
                        .with_usage_aggregator(usage_aggregator.clone())
                        .with_groups(group_manager.clone());
                if let Some(ts) = &trace_store {
                    admin_state = admin_state.with_trace_store(ts.clone());
                }
                admin_state.service.start_auto_update_scheduler();

                let admin_app = admin::create_admin_router(admin_state);

                // 创建 Admin UI 路由
                let admin_ui_app = admin_ui::create_admin_ui_router();

                tracing::info!("Admin API 已启用");
                tracing::info!("Admin UI 已启用: /admin");
                anthropic_app
                    .nest("/api/admin", admin_app)
                    .nest("/admin", admin_ui_app)
            }
        } else {
            anthropic_app
        }
    };

    // 启动服务器
    let addr = {
        let cfg = config.read();
        format!("{}:{}", cfg.host, cfg.port)
    };
    tracing::info!("启动 Anthropic API 端点: {}", addr);
    #[cfg(feature = "sensitive-logs")]
    tracing::debug!("API Key: {}***", &api_key[..(api_key.len() / 2)]);
    #[cfg(not(feature = "sensitive-logs"))]
    tracing::info!(
        "API Key: ***{} (长度: {})",
        &api_key[api_key.len().saturating_sub(4)..],
        api_key.len()
    );
    tracing::info!("可用 API:");
    tracing::info!("  POST /v1/messages              (Claude 兼容)");
    tracing::info!("  POST /anthropic/v1/messages     (Claude Code)");
    tracing::info!("  POST /v1/chat/completions       (OpenAI 兼容)");
    tracing::info!("  POST /v1/responses              (OpenAI Responses)");
    tracing::info!("  POST /v1beta/models/*:generateContent (Gemini 兼容)");
    tracing::info!("  GET  /v1/models");
    tracing::info!("  GET  /v1beta/models             (Gemini 模型)");
    tracing::info!("  POST /v1/messages/count_tokens");
    tracing::info!("  GET  /health");
    if admin_key_valid {
        tracing::info!("Admin API:");
        tracing::info!("  GET  /api/admin/credentials");
        tracing::info!("  POST /api/admin/credentials/:index/disabled");
        tracing::info!("  POST /api/admin/credentials/:index/priority");
        tracing::info!("  POST /api/admin/credentials/:index/reset");
        tracing::info!("  GET  /api/admin/credentials/:index/balance");
        tracing::info!("Admin UI:");
        tracing::info!("  GET  /admin");
    }

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("绑定监听地址失败 ({}): {}", addr, e);
            std::process::exit(1);
        });
    if let Err(e) = axum::serve(listener, app).await {
        tracing::error!("HTTP 服务异常退出: {}", e);
        std::process::exit(1);
    }
}
