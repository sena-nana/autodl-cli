use std::fs;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::GzEncoder;
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use russh_sftp::client::SftpSession;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::AppError;
use crate::ssh::SshSession;

pub const DEFAULT_EXCLUDES: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "__pycache__",
    ".venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
];

#[derive(Debug, Clone)]
pub struct PackOptions {
    pub use_gitignore: bool,
    pub use_default_excludes: bool,
    pub extra_excludes: Vec<String>,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            use_gitignore: true,
            use_default_excludes: true,
            extra_excludes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PackStats {
    pub files: usize,
    pub bytes_raw: u64,
    pub bytes_compressed: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_epoch: Option<u64>,
}

/// Escape an argument for POSIX `sh`/`bash` single-quoted context.
pub fn shell_escape(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:=@,+".contains(&b))
    {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('\'');
    for ch in arg.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Normalize a local relative path into POSIX forward-slash archive path.
fn to_archive_path(rel: &Path) -> Result<String, AppError> {
    let mut parts = Vec::new();
    for comp in rel.components() {
        if let std::path::Component::Normal(os_str) = comp {
            let s = os_str.to_str().ok_or_else(|| {
                AppError::Validation(format!(
                    "Non-UTF-8 path component in '{}'",
                    rel.display()
                ))
            })?;
            parts.push(s);
        }
    }
    Ok(parts.join("/"))
}

/// Pack a local directory into an in-memory `.tar.gz` buffer, respecting `.gitignore`,
/// `.autodlignore`, default dev/ML exclusions, and custom `--exclude` patterns.
pub fn pack_directory_tar_gz(
    root: &Path,
    opts: &PackOptions,
) -> Result<(Vec<u8>, PackStats), AppError> {
    if !root.is_dir() {
        return Err(AppError::Validation(format!(
            "Sync source '{}' is not a directory",
            root.display()
        )));
    }

    let mut overrides = OverrideBuilder::new(root);
    if opts.use_default_excludes {
        for pat in DEFAULT_EXCLUDES {
            overrides
                .add(&format!("!{pat}/"))
                .map_err(|e| AppError::Validation(format!("Invalid default exclude '{pat}': {e}")))?;
            overrides
                .add(&format!("!{pat}"))
                .map_err(|e| AppError::Validation(format!("Invalid default exclude '{pat}': {e}")))?;
        }
    }
    for pat in &opts.extra_excludes {
        let trimmed = pat.trim();
        if trimmed.is_empty() {
            continue;
        }
        let neg = if trimmed.starts_with('!') {
            trimmed.to_string()
        } else {
            format!("!{trimmed}")
        };
        overrides
            .add(&neg)
            .map_err(|e| AppError::Validation(format!("Invalid exclude pattern '{pat}': {e}")))?;
    }
    let override_matcher = overrides
        .build()
        .map_err(|e| AppError::Validation(format!("Failed to build exclude matcher: {e}")))?;

    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(opts.use_gitignore)
        .git_global(opts.use_gitignore)
        .git_exclude(opts.use_gitignore)
        .ignore(opts.use_gitignore)
        .add_custom_ignore_filename(".autodlignore")
        .overrides(override_matcher);

    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut files = 0usize;
    let mut bytes_raw = 0u64;

    {
        let mut tar_builder = tar::Builder::new(&mut encoder);
        tar_builder.follow_symlinks(false);

        for entry_res in walker.build() {
            let entry = entry_res.map_err(|e| AppError::Other(format!("Directory walk error: {e}")))?;
            let path = entry.path();
            if path == root {
                continue;
            }
            let rel = path.strip_prefix(root).map_err(|e| {
                AppError::Other(format!("Failed to compute relative path: {e}"))
            })?;
            let archive_rel = to_archive_path(rel)?;
            if archive_rel.is_empty() {
                continue;
            }

            let meta = entry
                .metadata()
                .map_err(|e| AppError::Other(format!("Metadata error for '{}': {e}", path.display())))?;

            if meta.is_file() {
                bytes_raw += meta.len();
                files += 1;
                tar_builder.append_path_with_name(path, &archive_rel)?;
            } else if meta.is_dir() {
                tar_builder.append_dir(&archive_rel, path)?;
            }
        }
        tar_builder.finish()?;
    }

    let compressed = encoder.finish()?;
    let stats = PackStats {
        files,
        bytes_raw,
        bytes_compressed: compressed.len() as u64,
    };
    Ok((compressed, stats))
}

/// Stream a packed `.tar.gz` code directory directly into `remote_dir` via SSH stdin.
pub async fn sync_directory(
    ssh: &SshSession,
    local_dir: &Path,
    remote_dir: &str,
    opts: &PackOptions,
    clean_remote_first: bool,
) -> Result<PackStats, AppError> {
    let (archive, stats) = pack_directory_tar_gz(local_dir, opts)?;
    let escaped_dir = shell_escape(remote_dir);
    let cmd = if clean_remote_first {
        format!("rm -rf {escaped_dir} && mkdir -p {escaped_dir} && tar -xzf - -C {escaped_dir}")
    } else {
        format!("mkdir -p {escaped_dir} && tar -xzf - -C {escaped_dir}")
    };

    let res = ssh
        .exec_streaming(&cmd, Some(archive), None, true)
        .await?;
    if res.exit_code != 0 {
        return Err(AppError::RemoteExit {
            code: res.exit_code,
            message: format!(
                "Failed extracting synced code archive into '{remote_dir}': {}",
                res.stderr.trim()
            ),
        });
    }
    Ok(stats)
}

/// Ensure a remote directory exists via SFTP (recursive `mkdir -p`).
pub async fn ensure_remote_dir(sftp: &SftpSession, remote_dir: &str) -> Result<(), AppError> {
    let trimmed = remote_dir.trim_end_matches('/');
    if trimmed.is_empty() || trimmed == "/" {
        return Ok(());
    }
    let mut current = if trimmed.starts_with('/') {
        String::from("/")
    } else {
        String::new()
    };
    for part in trimmed.split('/').filter(|p| !p.is_empty()) {
        if !current.is_empty() && !current.ends_with('/') {
            current.push('/');
        }
        current.push_str(part);
        let exists = sftp.try_exists(&current).await.unwrap_or(false);
        if !exists {
            let _ = sftp.create_dir(&current).await;
        }
    }
    Ok(())
}

/// Upload a single local file via SFTP to an exact remote file path.
pub async fn upload_file_sftp(
    sftp: &SftpSession,
    local_file: &Path,
    remote_file: &str,
) -> Result<u64, AppError> {
    let data = fs::read(local_file)?;
    let len = data.len() as u64;
    if let Some((parent, _)) = remote_file.rsplit_once('/')
        && !parent.is_empty()
    {
        ensure_remote_dir(sftp, parent).await?;
    }
    let mut remote = sftp
        .create(remote_file)
        .await
        .map_err(|e| AppError::Ssh(format!("SFTP create '{remote_file}' failed: {e}")))?;
    remote
        .write_all(&data)
        .await
        .map_err(|e| AppError::Ssh(format!("SFTP write '{remote_file}' failed: {e}")))?;
    remote
        .shutdown()
        .await
        .map_err(|e| AppError::Ssh(format!("SFTP flush '{remote_file}' failed: {e}")))?;
    Ok(len)
}

/// Upload a local file or directory to a remote path.
pub async fn upload_path(
    ssh: &SshSession,
    local_path: &Path,
    remote_path: &str,
    opts: &PackOptions,
) -> Result<PackStats, AppError> {
    if !local_path.exists() {
        return Err(AppError::NotFound(format!(
            "Local path '{}' does not exist",
            local_path.display()
        )));
    }
    if local_path.is_dir() {
        return sync_directory(ssh, local_path, remote_path, opts, false).await;
    }

    let sftp = ssh.open_sftp().await?;
    let target = if remote_path.ends_with('/') {
        let fname = local_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AppError::Validation("Invalid local file name".into()))?;
        format!("{remote_path}{fname}")
    } else if sftp
        .metadata(remote_path)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        let fname = local_path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AppError::Validation("Invalid local file name".into()))?;
        format!("{}/{fname}", remote_path.trim_end_matches('/'))
    } else {
        remote_path.to_string()
    };

    let bytes = upload_file_sftp(&sftp, local_path, &target).await?;
    let _ = sftp.close().await;
    Ok(PackStats {
        files: 1,
        bytes_raw: bytes,
        bytes_compressed: bytes,
    })
}

/// Download a remote file via SFTP to a local path.
pub async fn download_file_sftp(
    sftp: &SftpSession,
    remote_file: &str,
    local_path: &Path,
) -> Result<(PathBuf, u64), AppError> {
    let dest = if local_path.is_dir()
        || local_path
            .to_str()
            .map(|s| s.ends_with('/') || s.ends_with('\\'))
            .unwrap_or(false)
    {
        fs::create_dir_all(local_path)?;
        let fname = remote_file
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("downloaded_file");
        local_path.join(fname)
    } else {
        if let Some(parent) = local_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        local_path.to_path_buf()
    };

    let mut file = sftp
        .open(remote_file)
        .await
        .map_err(|e| AppError::Ssh(format!("SFTP open '{remote_file}' failed: {e}")))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .map_err(|e| AppError::Ssh(format!("SFTP read '{remote_file}' failed: {e}")))?;
    let len = buf.len() as u64;
    fs::write(&dest, &buf)?;
    Ok((dest, len))
}

/// Download a remote file or directory (`recursive = true` streams a tar.gz from the remote host).
pub async fn download_path(
    ssh: &SshSession,
    remote_path: &str,
    local_path: &Path,
    recursive: bool,
) -> Result<(PathBuf, u64), AppError> {
    let sftp = ssh.open_sftp().await?;
    let meta = sftp
        .metadata(remote_path)
        .await
        .map_err(|e| AppError::NotFound(format!("Remote path '{remote_path}' not found: {e}")))?;

    if meta.is_dir() {
        let _ = sftp.close().await;
        if !recursive {
            return Err(AppError::Validation(format!(
                "Remote path '{remote_path}' is a directory; pass -r/--recursive to download"
            )));
        }
        return download_directory_archive(ssh, remote_path, local_path).await;
    }

    let res = download_file_sftp(&sftp, remote_path, local_path).await;
    let _ = sftp.close().await;
    res
}

/// Stream a remote directory as tar.gz and unpack locally.
pub async fn download_directory_archive(
    ssh: &SshSession,
    remote_dir: &str,
    local_dir: &Path,
) -> Result<(PathBuf, u64), AppError> {
    fs::create_dir_all(local_dir)?;
    let cmd = format!("tar -czf - -C {} .", shell_escape(remote_dir));
    // Capture raw bytes via exec channel directly using base64 so binary tar stream isn't mangled by String conversion.
    let b64_cmd = format!("({cmd}) | base64 -w 0");
    let out = ssh.exec_streaming(&b64_cmd, None, None, true).await?;
    if out.exit_code != 0 {
        return Err(AppError::RemoteExit {
            code: out.exit_code,
            message: format!(
                "Failed archiving remote directory '{remote_dir}': {}",
                out.stderr.trim()
            ),
        });
    }
    use base64::Engine;
    let compressed = base64::engine::general_purpose::STANDARD
        .decode(out.stdout.trim())
        .map_err(|e| AppError::Ssh(format!("Failed decoding downloaded archive stream: {e}")))?;
    let len = compressed.len() as u64;
    let decoder = flate2::read::GzDecoder::new(&compressed[..]);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(local_dir)?;
    Ok((local_dir.to_path_buf(), len))
}

/// List entries in a remote directory via SFTP.
pub async fn sftp_ls(sftp: &SftpSession, remote_path: &str) -> Result<Vec<RemoteEntry>, AppError> {
    let dir = sftp
        .read_dir(remote_path)
        .await
        .map_err(|e| AppError::Ssh(format!("SFTP read_dir '{remote_path}' failed: {e}")))?;

    let mut entries = Vec::new();
    for entry in dir {
        let name = entry.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let meta = entry.metadata();
        let modified_epoch = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        entries.push(RemoteEntry {
            name,
            is_dir: meta.is_dir(),
            size: meta.len(),
            modified_epoch,
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

/// Remove a remote file (via SFTP) or directory recursively (`rm -rf` via SSH).
pub async fn remote_rm(
    ssh: &SshSession,
    remote_path: &str,
    recursive: bool,
) -> Result<(), AppError> {
    let trimmed = remote_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return Err(AppError::Validation("Refusing to remove '/'".into()));
    }
    if recursive {
        let cmd = format!("rm -rf -- {}", shell_escape(trimmed));
        let out = ssh.exec_streaming(&cmd, None, None, true).await?;
        if out.exit_code != 0 {
            return Err(AppError::RemoteExit {
                code: out.exit_code,
                message: out.stderr,
            });
        }
        return Ok(());
    }
    let sftp = ssh.open_sftp().await?;
    let res = sftp
        .remove_file(trimmed)
        .await
        .map_err(|e| AppError::Ssh(format!("Failed to remove '{trimmed}': {e}")));
    let _ = sftp.close().await;
    res
}
