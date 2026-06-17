//! 在线更新的二进制下载与替换实现。
//!
//! 这套方案不再操作 docker daemon：
//! 1. 从 GitHub Releases 下载与当前平台匹配的 `kiro-rs-<ver>-<plat>.tar.gz`/`.zip`；
//! 2. 验证 `SHA256SUMS.txt` 校验和；
//! 3. 解压取出新二进制，原子替换当前 exe，旧版本写到 `<exe>.backup`；
//! 4. 调用方收到 `need_restart=true` 后再触发进程退出，由 docker 的
//!    `restart: unless-stopped` 接管重启，新版本随之生效。
//!
//! 这样做的好处是更新过程完全不依赖容器自管自，网络断/校验失败时旧二进制
//! 仍然在跑，避免之前 docker compose pull 路径上的"旧停新挂"事故。

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::admin::error::AdminServiceError;

const MAX_DOWNLOAD_BYTES: u64 = 200 * 1024 * 1024;

const GITHUB_REPO: &str = "ZakiLiu/kiro.rs";

fn binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "kiro-rs.exe"
    } else {
        "kiro-rs"
    }
}

pub fn current_executable() -> Result<PathBuf, AdminServiceError> {
    let exe = std::env::current_exe().map_err(|e| {
        AdminServiceError::InternalError(format!("无法获取当前可执行文件路径: {}", e))
    })?;
    let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
    Ok(resolved)
}

pub fn backup_path(exe: &Path) -> PathBuf {
    let mut s = exe.as_os_str().to_os_string();
    s.push(".backup");
    PathBuf::from(s)
}

fn platform_suffix() -> Result<&'static str, AdminServiceError> {
    let suffix = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "Linux-musl-x64.tar.gz",
        ("linux", "aarch64") => "Linux-musl-arm64.tar.gz",
        ("macos", "x86_64") => "macOS-x64.tar.gz",
        ("macos", "aarch64") => "macOS-arm64.tar.gz",
        ("windows", "x86_64") => "Windows-x64.zip",
        (os, arch) => {
            return Err(AdminServiceError::InternalError(format!(
                "不支持的平台 {}/{}：在线更新仅支持 release.yaml 矩阵中已发布的目标",
                os, arch
            )));
        }
    };
    Ok(suffix)
}

fn archive_filename(version: &str) -> Result<String, AdminServiceError> {
    let v = version.trim().trim_start_matches('v');
    if v.is_empty() {
        return Err(AdminServiceError::InternalError(
            "版本号为空，无法定位 release 资产".to_string(),
        ));
    }
    Ok(format!("kiro-rs-{}-{}", v, platform_suffix()?))
}

pub async fn download_release_binary(
    version: &str,
    proxy: Option<&str>,
    github_token: Option<&str>,
    dest: &Path,
) -> Result<(), AdminServiceError> {
    let archive = archive_filename(version)?;
    let base = format!(
        "https://github.com/{}/releases/download/v{}",
        GITHUB_REPO,
        version.trim().trim_start_matches('v')
    );
    let archive_url = format!("{}/{}", base, archive);
    let checksums_url = format!("{}/SHA256SUMS.txt", base);

    let client = build_http_client(proxy)?;
    let token = github_token.and_then(|t| {
        let trimmed = t.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "kiro-rs-update-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    fs::create_dir_all(&tmp_dir).map_err(|e| {
        AdminServiceError::InternalError(format!("创建更新临时目录失败: {}", e))
    })?;
    let archive_path = tmp_dir.join(&archive);

    download_to_file(&client, &archive_url, token.as_deref(), &archive_path).await?;
    verify_checksum(
        &client,
        &checksums_url,
        token.as_deref(),
        &archive,
        &archive_path,
    )
    .await?;

    let extract_dir = tmp_dir.join("extract");
    fs::create_dir_all(&extract_dir).map_err(|e| {
        AdminServiceError::InternalError(format!("创建解压目录失败: {}", e))
    })?;
    extract_archive(&archive_path, &extract_dir)?;

    let extracted = locate_binary(&extract_dir)?;
    fs::copy(&extracted, dest).map_err(|e| {
        AdminServiceError::InternalError(format!("拷贝新二进制失败: {}", e))
    })?;
    set_executable(dest)?;

    let _ = fs::remove_dir_all(&tmp_dir);
    Ok(())
}

pub(super) fn build_http_client(
    proxy: Option<&str>,
) -> Result<reqwest::Client, AdminServiceError> {
    let mut builder = reqwest::Client::builder()
        .user_agent("kiro-rs-updater")
        .timeout(std::time::Duration::from_secs(180));
    if let Some(url) = proxy.and_then(|u| {
        let s = u.trim();
        if s.is_empty() { None } else { Some(s) }
    }) {
        let proxy = reqwest::Proxy::all(url).map_err(|e| {
            AdminServiceError::InternalError(format!("代理配置无效: {}", e))
        })?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| AdminServiceError::InternalError(format!("构造 HTTP 客户端失败: {}", e)))
}

async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    token: Option<&str>,
    dest: &Path,
) -> Result<(), AdminServiceError> {
    let mut req = client.get(url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let resp = req.send().await.map_err(|e| {
        AdminServiceError::InternalError(format!("下载 {} 失败: {}", url, e))
    })?;
    if !resp.status().is_success() {
        return Err(AdminServiceError::InternalError(format!(
            "下载 {} 返回 {}",
            url,
            resp.status()
        )));
    }
    if let Some(len) = resp.content_length() {
        if len > MAX_DOWNLOAD_BYTES {
            return Err(AdminServiceError::InternalError(format!(
                "下载体积 {} 字节超过上限 {} 字节",
                len, MAX_DOWNLOAD_BYTES
            )));
        }
    }

    let bytes = resp.bytes().await.map_err(|e| {
        AdminServiceError::InternalError(format!("读取下载内容失败: {}", e))
    })?;
    if bytes.len() as u64 > MAX_DOWNLOAD_BYTES {
        return Err(AdminServiceError::InternalError(format!(
            "实际下载体积 {} 字节超过上限",
            bytes.len()
        )));
    }
    fs::write(dest, &bytes).map_err(|e| {
        AdminServiceError::InternalError(format!("写入下载文件 {} 失败: {}", dest.display(), e))
    })?;
    Ok(())
}

async fn verify_checksum(
    client: &reqwest::Client,
    checksums_url: &str,
    token: Option<&str>,
    archive_name: &str,
    archive_path: &Path,
) -> Result<(), AdminServiceError> {
    let mut req = client.get(checksums_url);
    if let Some(t) = token {
        req = req.header("Authorization", format!("Bearer {}", t));
    }
    let resp = req.send().await.map_err(|e| {
        AdminServiceError::InternalError(format!("下载 SHA256SUMS.txt 失败: {}", e))
    })?;
    if !resp.status().is_success() {
        return Err(AdminServiceError::InternalError(format!(
            "下载 SHA256SUMS.txt 返回 {}",
            resp.status()
        )));
    }
    let body = resp.text().await.map_err(|e| {
        AdminServiceError::InternalError(format!("读取 SHA256SUMS.txt 失败: {}", e))
    })?;

    let expected = body
        .lines()
        .filter_map(|line| {
            let mut iter = line.split_whitespace();
            let hash = iter.next()?;
            let name = iter.next()?.trim_start_matches('*');
            if name == archive_name {
                Some(hash.to_ascii_lowercase())
            } else {
                None
            }
        })
        .next()
        .ok_or_else(|| {
            AdminServiceError::InternalError(format!(
                "SHA256SUMS.txt 中未找到 {} 的校验项",
                archive_name
            ))
        })?;

    let actual = sha256_file(archive_path)?;
    if actual != expected {
        return Err(AdminServiceError::InternalError(format!(
            "校验和不匹配：期望 {}，实际 {}",
            expected, actual
        )));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, AdminServiceError> {
    let mut file = fs::File::open(path).map_err(|e| {
        AdminServiceError::InternalError(format!("打开 {} 失败: {}", path.display(), e))
    })?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| {
            AdminServiceError::InternalError(format!("读取 {} 失败: {}", path.display(), e))
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<(), AdminServiceError> {
    let name = archive
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(archive, dest)
    } else if name.ends_with(".zip") {
        extract_zip(archive, dest)
    } else {
        Err(AdminServiceError::InternalError(format!(
            "不支持的归档格式: {}",
            name
        )))
    }
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<(), AdminServiceError> {
    let bytes = fs::read(archive).map_err(|e| {
        AdminServiceError::InternalError(format!("读取归档 {} 失败: {}", archive.display(), e))
    })?;
    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut tar = tar::Archive::new(gz);
    tar.unpack(dest).map_err(|e| {
        AdminServiceError::InternalError(format!("解压 tar.gz 失败: {}", e))
    })
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<(), AdminServiceError> {
    let file = fs::File::open(archive).map_err(|e| {
        AdminServiceError::InternalError(format!("打开 {} 失败: {}", archive.display(), e))
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| {
        AdminServiceError::InternalError(format!("解析 zip 失败: {}", e))
    })?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| {
            AdminServiceError::InternalError(format!("读取 zip 条目失败: {}", e))
        })?;
        let entry_path = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue,
        };
        let target = dest.join(entry_path);
        if entry.is_dir() {
            fs::create_dir_all(&target).map_err(|e| {
                AdminServiceError::InternalError(format!("创建目录失败: {}", e))
            })?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                AdminServiceError::InternalError(format!("创建目录失败: {}", e))
            })?;
        }
        let mut out = fs::File::create(&target).map_err(|e| {
            AdminServiceError::InternalError(format!("创建文件 {} 失败: {}", target.display(), e))
        })?;
        std::io::copy(&mut entry, &mut out).map_err(|e| {
            AdminServiceError::InternalError(format!("写入 {} 失败: {}", target.display(), e))
        })?;
    }
    Ok(())
}

fn locate_binary(root: &Path) -> Result<PathBuf, AdminServiceError> {
    let target = binary_name();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|e| {
            AdminServiceError::InternalError(format!("读取目录 {} 失败: {}", dir.display(), e))
        })?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                AdminServiceError::InternalError(format!("枚举目录项失败: {}", e))
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n == target)
                .unwrap_or(false)
            {
                return Ok(path);
            }
        }
    }
    Err(AdminServiceError::InternalError(format!(
        "归档中未找到 {} 二进制",
        target
    )))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), AdminServiceError> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)
        .map_err(|e| AdminServiceError::InternalError(format!("读取权限失败: {}", e)))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).map_err(|e| {
        AdminServiceError::InternalError(format!("设置可执行权限失败: {}", e))
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<(), AdminServiceError> {
    Ok(())
}

pub fn install_binary(exe: &Path, staged: &Path) -> Result<(), AdminServiceError> {
    let backup = backup_path(exe);
    let _ = fs::remove_file(&backup);
    fs::rename(exe, &backup).map_err(|e| {
        AdminServiceError::InternalError(format!(
            "备份当前可执行文件 {} 失败: {}",
            exe.display(),
            e
        ))
    })?;
    if let Err(e) = fs::rename(staged, exe) {
        let _ = fs::rename(&backup, exe);
        return Err(AdminServiceError::InternalError(format!(
            "安装新可执行文件失败: {}",
            e
        )));
    }
    Ok(())
}

pub fn restore_backup(exe: &Path) -> Result<(), AdminServiceError> {
    let backup = backup_path(exe);
    if !backup.exists() {
        return Err(AdminServiceError::InternalError(
            "未找到本地备份二进制（<exe>.backup 不存在），无法离线回退".to_string(),
        ));
    }
    let mut rollback_tmp = exe.as_os_str().to_os_string();
    rollback_tmp.push(".rollback-current");
    let rollback_tmp = PathBuf::from(rollback_tmp);
    let _ = fs::remove_file(&rollback_tmp);
    fs::rename(exe, &rollback_tmp).map_err(|e| {
        AdminServiceError::InternalError(format!("暂存当前 exe 失败: {}", e))
    })?;
    if let Err(e) = fs::rename(&backup, exe) {
        let _ = fs::rename(&rollback_tmp, exe);
        return Err(AdminServiceError::InternalError(format!(
            "回退失败: {}",
            e
        )));
    }
    let _ = fs::remove_file(&rollback_tmp);
    Ok(())
}

pub fn schedule_self_exit(delay: std::time::Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        let _ = std::io::stdout().flush();
        std::process::exit(0);
    });
}
