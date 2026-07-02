//! Kiro CLI SQLite 凭据导入。
//!
//! `kiro-cli` / Amazon Q Developer CLI 会把登录态写进本机 SQLite。
//! 本模块只做启动期只读导入，转换成运行时凭据交给现有 MultiTokenManager，
//! 因而不改变 credentials.json、cooldown、cache 和 failover 链路。

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Context;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;

use crate::kiro::endpoint::CLI_ENDPOINT_NAME;
use crate::kiro::model::credentials::KiroCredentials;

pub const KIRO_CLI_DB_FILE_ENV: &str = "KIRO_CLI_DB_FILE";

const TOKEN_KEYS: &[(&str, CliDbAuthKind)] = &[
    ("kirocli:external_idp:token", CliDbAuthKind::ExternalIdp),
    ("kirocli:social:token", CliDbAuthKind::Social),
    ("kirocli:odic:token", CliDbAuthKind::Oidc),
    ("codewhisperer:odic:token", CliDbAuthKind::Oidc),
];

const REGISTRATION_KEYS: &[&str] = &[
    "kirocli:odic:device-registration",
    "codewhisperer:odic:device-registration",
];

const PROFILE_STATE_KEY: &str = "api.codewhisperer.profile";

#[derive(Debug, Clone)]
pub struct LoadedCliDbCredentials {
    pub credentials: KiroCredentials,
    pub db_path: PathBuf,
    pub token_key: String,
    pub has_device_registration: bool,
    pub has_state_profile: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliDbAuthKind {
    Social,
    Oidc,
    ExternalIdp,
}

impl CliDbAuthKind {
    fn auth_method(self) -> &'static str {
        match self {
            Self::Social => "social",
            Self::Oidc => "idc",
            Self::ExternalIdp => "external_idp",
        }
    }
}

#[derive(Debug, Deserialize)]
struct CliTokenRecord {
    access_token: Option<String>,
    refresh_token: Option<String>,
    profile_arn: Option<String>,
    region: Option<String>,
    expires_at: Option<String>,
    provider: Option<String>,
    token_endpoint: Option<String>,
    issuer_url: Option<String>,
    scopes: Option<String>,
    client_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CliDeviceRegistration {
    client_id: Option<String>,
    client_secret: Option<String>,
    region: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CliProfileState {
    arn: Option<String>,
}

/// 按优先级加载 Kiro CLI SQLite 凭据。
///
/// 优先级：
/// 1. `KIRO_CLI_DB_FILE`
/// 2. Windows `%LOCALAPPDATA%\Kiro-Cli\data.sqlite3`
/// 3. Linux/macOS `~/.local/share/kiro-cli/data.sqlite3`
/// 4. Amazon Q `~/.local/share/amazon-q/data.sqlite3`
pub fn load_from_env_or_default_paths() -> anyhow::Result<Option<LoadedCliDbCredentials>> {
    if let Ok(raw) = env::var(KIRO_CLI_DB_FILE_ENV)
        && !raw.trim().is_empty()
    {
        let path = expand_user_path(raw.trim());
        return load_from_path(&path).with_context(|| {
            format!(
                "读取 {} 指定的 Kiro CLI SQLite 失败: {}",
                KIRO_CLI_DB_FILE_ENV,
                path.display()
            )
        });
    }

    for path in default_db_paths() {
        if !path.exists() {
            continue;
        }
        if let Some(loaded) = load_from_path(&path)
            .with_context(|| format!("读取 Kiro CLI SQLite 失败: {}", path.display()))?
        {
            return Ok(Some(loaded));
        }
    }

    Ok(None)
}

pub fn load_from_path(path: &Path) -> anyhow::Result<Option<LoadedCliDbCredentials>> {
    if !path.exists() {
        return Ok(None);
    }

    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("打开 Kiro CLI SQLite 失败: {}", path.display()))?;

    let Some((token_key, auth_kind, token)) = load_first_token(&conn)? else {
        return Ok(None);
    };
    let registration = load_device_registration(&conn)?;
    let profile_state = load_profile_state(&conn)?;

    let profile_arn = non_empty(token.profile_arn).or_else(|| {
        profile_state
            .as_ref()
            .and_then(|profile| non_empty(profile.arn.clone()))
    });
    let api_region = profile_arn.as_deref().and_then(region_from_profile_arn);
    let provider = non_empty(token.provider);

    // provider 推断：如果 token 含 external_idp 相关 provider，覆盖 auth_kind
    let effective_auth_method = if auth_kind == CliDbAuthKind::ExternalIdp {
        "external_idp"
    } else if let Some(ref p) = provider {
        let pl = p.to_lowercase();
        if pl.contains("external") || pl.contains("azure") || pl.contains("entra") {
            "external_idp"
        } else {
            auth_kind.auth_method()
        }
    } else {
        auth_kind.auth_method()
    };

    let mut credentials = KiroCredentials {
        runtime_only: true,
        access_token: non_empty(token.access_token),
        refresh_token: non_empty(token.refresh_token),
        profile_arn,
        expires_at: non_empty(token.expires_at),
        auth_method: Some(effective_auth_method.to_string()),
        client_id: non_empty(token.client_id).or_else(|| {
            registration
                .as_ref()
                .and_then(|registration| non_empty(registration.client_id.clone()))
        }),
        client_secret: registration
            .as_ref()
            .and_then(|registration| non_empty(registration.client_secret.clone())),
        token_endpoint: non_empty(token.token_endpoint),
        issuer_url: non_empty(token.issuer_url),
        scopes: non_empty(token.scopes),
        region: non_empty(token.region).or_else(|| {
            registration
                .as_ref()
                .and_then(|registration| non_empty(registration.region.clone()))
        }),
        api_region,
        endpoint: Some(CLI_ENDPOINT_NAME.to_string()),
        source_channel: Some(
            provider
                .map(|provider| format!("kiro-cli-sqlite:{provider}"))
                .unwrap_or_else(|| "kiro-cli-sqlite".to_string()),
        ),
        ..Default::default()
    };
    credentials.canonicalize_auth_method();

    Ok(Some(LoadedCliDbCredentials {
        credentials,
        db_path: path.to_path_buf(),
        token_key: token_key.to_string(),
        has_device_registration: registration.is_some(),
        has_state_profile: profile_state
            .as_ref()
            .and_then(|profile| non_empty(profile.arn.clone()))
            .is_some(),
    }))
}

fn load_first_token(
    conn: &Connection,
) -> anyhow::Result<Option<(&'static str, CliDbAuthKind, CliTokenRecord)>> {
    for (key, kind) in TOKEN_KEYS {
        if let Some(raw) =
            query_value(conn, "auth_kv", key).with_context(|| format!("读取 auth_kv.{key} 失败"))?
        {
            let token: CliTokenRecord = serde_json::from_str(&raw)
                .with_context(|| format!("解析 auth_kv.{key} JSON 失败"))?;
            return Ok(Some((key, *kind, token)));
        }
    }

    Ok(None)
}

fn load_device_registration(conn: &Connection) -> anyhow::Result<Option<CliDeviceRegistration>> {
    for key in REGISTRATION_KEYS {
        if let Some(raw) =
            query_value(conn, "auth_kv", key).with_context(|| format!("读取 auth_kv.{key} 失败"))?
        {
            let registration = serde_json::from_str(&raw)
                .with_context(|| format!("解析 auth_kv.{key} JSON 失败"))?;
            return Ok(Some(registration));
        }
    }

    Ok(None)
}

fn load_profile_state(conn: &Connection) -> anyhow::Result<Option<CliProfileState>> {
    let Some(raw) = query_value(conn, "state", PROFILE_STATE_KEY)
        .or_else(|err| {
            if is_missing_table_error(&err) {
                Ok(None)
            } else {
                Err(err)
            }
        })
        .context("读取 state.api.codewhisperer.profile 失败")?
    else {
        return Ok(None);
    };

    let profile =
        serde_json::from_str(&raw).context("解析 state.api.codewhisperer.profile JSON 失败")?;
    Ok(Some(profile))
}

fn query_value(conn: &Connection, table: &str, key: &str) -> rusqlite::Result<Option<String>> {
    let sql = match table {
        "auth_kv" => "SELECT value FROM auth_kv WHERE key = ?1",
        "state" => "SELECT value FROM state WHERE key = ?1",
        _ => unreachable!("query_value only supports known local tables"),
    };
    conn.query_row(sql, [key], |row| row.get::<_, String>(0))
        .optional()
}

fn is_missing_table_error(err: &rusqlite::Error) -> bool {
    err.to_string().contains("no such table")
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn region_from_profile_arn(arn: &str) -> Option<String> {
    let region = arn.split(':').nth(3)?;
    if looks_like_region(region) {
        Some(region.to_string())
    } else {
        None
    }
}

fn looks_like_region(value: &str) -> bool {
    let mut parts = value.split('-');
    let first = parts.next().unwrap_or_default();
    let second = parts.next().unwrap_or_default();
    let third = parts.next().unwrap_or_default();
    parts.next().is_none()
        && !first.is_empty()
        && !second.is_empty()
        && third.chars().all(|c| c.is_ascii_digit())
        && !third.is_empty()
}

fn default_db_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(local_app_data) = env::var("LOCALAPPDATA")
        && !local_app_data.trim().is_empty()
    {
        paths.push(
            PathBuf::from(local_app_data)
                .join("Kiro-Cli")
                .join("data.sqlite3"),
        );
    }

    if let Some(home) = home_dir() {
        paths.push(
            home.join(".local")
                .join("share")
                .join("kiro-cli")
                .join("data.sqlite3"),
        );
        paths.push(
            home.join(".local")
                .join("share")
                .join("amazon-q")
                .join("data.sqlite3"),
        );
    }

    paths
}

fn expand_user_path(raw: &str) -> PathBuf {
    let trimmed = raw.trim_matches('"').trim_matches('\'');
    if trimmed == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(trimmed));
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
        && let Some(home) = home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(trimmed)
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use serde_json::json;

    fn temp_db_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("kiro-rs-{name}-{}.sqlite3", uuid::Uuid::new_v4()))
    }

    fn create_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        conn.execute(
            "CREATE TABLE auth_kv (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn.execute("CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT)", [])
            .unwrap();
        conn
    }

    #[test]
    fn test_loads_social_token_from_kiro_cli_sqlite() {
        let path = temp_db_path("social");
        let conn = create_db(&path);
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
            params![
                "kirocli:social:token",
                json!({
                    "access_token": "access-social",
                    "refresh_token": "refresh-social",
                    "expires_at": "2999-01-01T00:00:00.123456789Z",
                    "provider": "google",
                    "profile_arn": "arn:aws:codewhisperer:eu-central-1:123456789012:profile/SOCIAL"
                })
                .to_string()
            ],
        )
        .unwrap();
        drop(conn);

        let loaded = load_from_path(&path).unwrap().unwrap();
        assert_eq!(loaded.token_key, "kirocli:social:token");
        assert!(!loaded.has_device_registration);
        assert!(!loaded.has_state_profile);
        assert!(loaded.credentials.runtime_only);
        assert_eq!(loaded.credentials.endpoint.as_deref(), Some("cli"));
        assert_eq!(loaded.credentials.auth_method.as_deref(), Some("social"));
        assert_eq!(
            loaded.credentials.access_token.as_deref(),
            Some("access-social")
        );
        assert_eq!(
            loaded.credentials.refresh_token.as_deref(),
            Some("refresh-social")
        );
        assert_eq!(
            loaded.credentials.profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:eu-central-1:123456789012:profile/SOCIAL")
        );
        assert_eq!(
            loaded.credentials.api_region.as_deref(),
            Some("eu-central-1")
        );
        assert_eq!(
            loaded.credentials.source_channel.as_deref(),
            Some("kiro-cli-sqlite:google")
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_loads_oidc_registration_and_state_profile() {
        let path = temp_db_path("oidc");
        let conn = create_db(&path);
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
            params![
                "kirocli:odic:token",
                json!({
                    "access_token": "access-oidc",
                    "refresh_token": "refresh-oidc",
                    "expires_at": "2999-01-01T00:00:00Z",
                    "region": "us-west-2"
                })
                .to_string()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO auth_kv (key, value) VALUES (?1, ?2)",
            params![
                "kirocli:odic:device-registration",
                json!({
                    "client_id": "client-id",
                    "client_secret": "client-secret",
                    "region": "us-west-2"
                })
                .to_string()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)",
            params![
                PROFILE_STATE_KEY,
                json!({
                    "arn": "arn:aws:codewhisperer:ap-southeast-1:123456789012:profile/OIDC",
                    "profile_name": "Default"
                })
                .to_string()
            ],
        )
        .unwrap();
        drop(conn);

        let loaded = load_from_path(&path).unwrap().unwrap();
        assert_eq!(loaded.token_key, "kirocli:odic:token");
        assert!(loaded.has_device_registration);
        assert!(loaded.has_state_profile);
        assert_eq!(loaded.credentials.auth_method.as_deref(), Some("idc"));
        assert_eq!(loaded.credentials.client_id.as_deref(), Some("client-id"));
        assert_eq!(
            loaded.credentials.client_secret.as_deref(),
            Some("client-secret")
        );
        assert_eq!(loaded.credentials.region.as_deref(), Some("us-west-2"));
        assert_eq!(
            loaded.credentials.profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:ap-southeast-1:123456789012:profile/OIDC")
        );
        assert_eq!(
            loaded.credentials.api_region.as_deref(),
            Some("ap-southeast-1")
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_missing_token_returns_none() {
        let path = temp_db_path("empty");
        let conn = create_db(&path);
        drop(conn);

        assert!(load_from_path(&path).unwrap().is_none());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_region_from_profile_arn_validates_region_shape() {
        assert_eq!(
            region_from_profile_arn("arn:aws:codewhisperer:us-east-1:123:profile/X").as_deref(),
            Some("us-east-1")
        );
        assert!(
            region_from_profile_arn("arn:aws:codewhisperer:not-a-region:123:profile/X").is_none()
        );
    }
}
