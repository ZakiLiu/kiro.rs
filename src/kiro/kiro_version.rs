//! Kiro IDE 版本自动获取
//!
//! 从官方稳定版元数据端点读取 `currentRelease` 字段，得到当前发布的 Kiro IDE 版本号，
//! 用于构造与官方 IDE 一致的 User-Agent（`KiroIDE-<version>-<machineId>`）。
//!
//! - 进程内缓存（`OnceLock<RwLock<Option<String>>>`）+ 后台定时刷新；
//! - 跨平台 `currentRelease` 一致，固定使用可访问的 Linux 元数据；
//! - 获取失败时调用方回退到 `config.kiro_version`，不阻塞启动。
//!
//! 用量类 REST 接口继续固定使用兼容版本，不使用这里的最新版本。

use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::RwLock;
use serde::Deserialize;

use crate::http_client::{ProxyConfig, build_client};
use crate::model::config::TlsBackend;

const METADATA_URL: &str =
    "https://prod.download.desktop.kiro.dev/stable/metadata-linux-x64-stable.json";

/// 用量类接口固定使用的 Kiro IDE 兼容版本。
pub const USAGE_API_KIRO_VERSION: &str = "0.9.2";

static LATEST_VERSION: OnceLock<RwLock<Option<String>>> = OnceLock::new();

fn cell() -> &'static RwLock<Option<String>> {
    LATEST_VERSION.get_or_init(|| RwLock::new(None))
}

/// 返回后台刷新成功后缓存的最新 Kiro IDE 版本。
pub fn cached() -> Option<String> {
    cell().read().clone()
}

/// 优先返回自动获取的最新版本，否则返回配置中的回退版本。
pub fn effective(fallback: &str) -> String {
    cached().unwrap_or_else(|| fallback.to_string())
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(rename = "currentRelease")]
    current_release: Option<String>,
}

/// 拉取一次最新 Kiro IDE 版本。
pub async fn fetch_latest(
    proxy: Option<&ProxyConfig>,
    tls_backend: TlsBackend,
) -> anyhow::Result<String> {
    let client = build_client(proxy, 15, tls_backend)?;
    let response = client.get(METADATA_URL).send().await?;
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("获取 Kiro 版本元数据失败: {}", status);
    }

    let metadata: Metadata = response.json().await?;
    metadata
        .current_release
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("元数据缺少 currentRelease"))
}

/// 启动后台刷新任务：立即拉取一次，之后按 `interval` 周期刷新。
pub fn spawn_refresher(proxy: Option<ProxyConfig>, tls_backend: TlsBackend, interval: Duration) {
    tokio::spawn(async move {
        loop {
            match fetch_latest(proxy.as_ref(), tls_backend).await {
                Ok(version) => {
                    let changed = cached().as_deref() != Some(version.as_str());
                    *cell().write() = Some(version.clone());
                    if changed {
                        tracing::info!("已自动获取 Kiro IDE 版本: {}", version);
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        "自动获取 Kiro IDE 版本失败（继续使用配置中的版本）: {}",
                        error
                    );
                }
            }
            tokio::time::sleep(interval).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_parses_current_release() {
        let metadata: Metadata =
            serde_json::from_str(r#"{"currentRelease":"0.12.301","releases":[]}"#).unwrap();
        assert_eq!(metadata.current_release.as_deref(), Some("0.12.301"));
    }

    #[test]
    fn test_effective_returns_non_empty_version() {
        assert!(!effective("0.9.2").is_empty());
    }
}
