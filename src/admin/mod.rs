//! Admin API 模块
//!
//! 提供凭据管理和监控功能的 HTTP API
//!
//! # 功能
//! - 查询所有凭据状态
//! - 启用/禁用凭据
//! - 修改凭据优先级
//! - 重置失败计数
//! - 查询凭据余额

pub(crate) mod binary_update;
pub mod client_keys;
mod error;
pub mod groups;
mod handlers;
mod middleware;
pub mod proxy_pool;
mod router;
mod service;
pub mod trace_db;
pub mod types;
pub mod usage_stats;

pub use middleware::AdminState;
pub use router::create_admin_router;
pub use service::AdminService;
