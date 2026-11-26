use serde::Serialize;

use crate::error::AppError;
use crate::ssh::SshSession;
use crate::ssh::transfer::shell_escape;

pub const AUTODL_FS_ROOT: &str = "/root/autodl-fs";
pub const AUTODL_TMP_ROOT: &str = "/root/autodl-tmp";
pub const AUTODL_FS_INODE_LIMIT: u64 = 200_000;

#[derive(Debug, Clone, Serialize)]
pub struct DiskMountStat {
    pub path: String,
    pub filesystem: String,
    pub mount_point: String,
    pub exists: bool,
    pub mounted_separately: bool,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub avail_bytes: u64,
    pub use_percent: f64,
    pub inodes_total: Option<u64>,
    pub inodes_used: Option<u64>,
    pub inodes_free: Option<u64>,
    pub inodes_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DuEntry {
    pub bytes: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HiddenCacheEntry {
    pub path: String,
    pub size_bytes: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CleanCacheReport {
    pub mounted: bool,
    pub dry_run: bool,
    pub items: Vec<HiddenCacheEntry>,
    pub reclaimed_bytes: u64,
    pub removed_count: usize,
}

/// Resolve a user-supplied path into `/root/autodl-fs` (preventing path traversal outside it).
pub fn resolve_fs_path(input: Option<&str>) -> Result<String, AppError> {
    let raw = input.map(str::trim).unwrap_or("");
    if raw.is_empty() || raw == "." || raw == "/" {
        return Ok(AUTODL_FS_ROOT.to_string());
    }

    let normalized_input = raw.replace('\\', "/");
    let sub = if normalized_input == AUTODL_FS_ROOT {
        ""
    } else if let Some(rest) = normalized_input.strip_prefix("/root/autodl-fs/") {
        rest
    } else {
        normalized_input.trim_start_matches('/')
    };

    let mut stack: Vec<&str> = Vec::new();
    for seg in sub.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Err(AppError::Validation(format!(
                        "Path '{raw}' escapes {AUTODL_FS_ROOT}"
                    )));
                }
            }
            s => stack.push(s),
        }
    }

    if stack.is_empty() {
        Ok(AUTODL_FS_ROOT.to_string())
    } else {
        Ok(format!("{AUTODL_FS_ROOT}/{}", stack.join("/")))
    }
}

/// Reject destructive operations targeting `/root/autodl-fs` itself.
pub fn ensure_not_fs_root(resolved: &str) -> Result<(), AppError> {
    if resolved.trim_end_matches('/') == AUTODL_FS_ROOT {
        return Err(AppError::Validation(format!(
            "Refusing to delete file storage root '{AUTODL_FS_ROOT}'"
        )));
    }
    Ok(())
}

/// Parse `df -PB1 <path>` output into `(filesystem, total, used, avail, use_pct, mount_point)`.
pub fn parse_df_bytes_output(output: &str) -> Option<(String, u64, u64, u64, f64, String)> {
    let line = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .nth(1)?;

    // Split first 5 whitespace-separated fields while preserving spaces in mount_point.
    let mut fields: Vec<&str> = Vec::with_capacity(5);
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start();
        if rest.is_empty() {
            return None;
        }
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = &rest[end..];
    }
    let mount_point = rest.trim().to_string();
    if mount_point.is_empty() {
        return None;
    }

    let fs = fields[0].to_string();
    let total = fields[1].parse::<u64>().ok()?;
    let used = fields[2].parse::<u64>().ok()?;
    let avail = fields[3].parse::<u64>().ok()?;
    let pct_str = fields[4].trim_end_matches('%');
    let pct = pct_str.parse::<f64>().unwrap_or_else(|_| {
        if total > 0 {
            (used as f64 / total as f64) * 100.0
        } else {
            0.0
        }
    });
    Some((fs, total, used, avail, pct, mount_point))
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct InodeStats {
    pub total: Option<u64>,
    pub used: Option<u64>,
    pub free: Option<u64>,
    pub percent: Option<f64>,
}

/// Parse `df -Pi <path>` output into `InodeStats`.
/// Handles `-` values for filesystems without fixed inode tables.
pub fn parse_df_inodes_output(output: &str) -> Option<InodeStats> {
    let line = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .nth(1)?;

    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 5 {
        return None;
    }
    let total = parse_opt_u64(cols[1]);
    let used = parse_opt_u64(cols[2]);
    let free = parse_opt_u64(cols[3]);
    let percent = cols[4].trim_end_matches('%').parse::<f64>().ok().or_else(|| {
        match (total, used) {
            (Some(t), Some(u)) if t > 0 => Some((u as f64 / t as f64) * 100.0),
            _ => None,
        }
    });
    Some(InodeStats {
        total,
        used,
        free,
        percent,
    })
}

fn parse_opt_u64(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if trimmed == "-" || trimmed.is_empty() {
        None
    } else {
        trimmed.parse::<u64>().ok()
    }
}

/// Parse `du -B1` lines (`<bytes>\t<path>`).
pub fn parse_du_output(output: &str) -> Vec<DuEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let split = trimmed
            .split_once('\t')
            .or_else(|| trimmed.split_once(' '));
        if let Some((size_s, path_s)) = split
            && let Ok(bytes) = size_s.trim().parse::<u64>()
        {
            entries.push(DuEntry {
                bytes,
                path: path_s.trim().to_string(),
            });
        }
    }
    entries
}

/// Parse `<d|f>\t<bytes>\t<path>` lines from hidden cache enumeration.
pub fn parse_cache_list_output(output: &str) -> Vec<HiddenCacheEntry> {
    let mut entries = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.splitn(3, '\t').collect();
        if parts.len() == 3
            && let Ok(size_bytes) = parts[1].trim().parse::<u64>()
        {
            entries.push(HiddenCacheEntry {
                is_dir: parts[0].trim() == "d",
                size_bytes,
                path: parts[2].trim().to_string(),
            });
        }
    }
    entries
}

/// Inspect existence, separate mount status, byte capacity, and inode usage of `target`.
pub async fn inspect_disk_path(
    ssh: &SshSession,
    target: &str,
) -> Result<DiskMountStat, AppError> {
    let esc = shell_escape(target);
    let cmd = format!(
        concat!(
            "if [ -e {0} ]; then echo '__EXISTS__=1'; else echo '__EXISTS__=0'; fi; ",
            "if command -v mountpoint >/dev/null 2>&1; then ",
            "if mountpoint -q {0}; then echo '__MOUNTED__=1'; else echo '__MOUNTED__=0'; fi; ",
            "else ",
            "d1=$(stat -c %d {0} 2>/dev/null || echo ''); ",
            "d2=$(stat -c %d \"$(dirname {0})\" 2>/dev/null || echo ''); ",
            "if [ -n \"$d1\" ] && [ -n \"$d2\" ] && [ \"$d1\" != \"$d2\" ]; then echo '__MOUNTED__=1'; else echo '__MOUNTED__=0'; fi; ",
            "fi; ",
            "echo '__DF_BYTES__'; df -PB1 {0} 2>/dev/null || true; ",
            "echo '__DF_INODES__'; df -Pi {0} 2>/dev/null || true"
        ),
        esc
    );

    let out = ssh.exec_streaming(&cmd, None, None, true).await?;
    let mut exists = false;
    let mut mounted_separately = false;
    let mut df_bytes_lines = Vec::new();
    let mut df_inodes_lines = Vec::new();
    let mut section = "";

    for line in out.stdout.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("__EXISTS__=") {
            exists = val.trim() == "1";
        } else if let Some(val) = trimmed.strip_prefix("__MOUNTED__=") {
            mounted_separately = val.trim() == "1";
        } else if trimmed == "__DF_BYTES__" {
            section = "bytes";
        } else if trimmed == "__DF_INODES__" {
            section = "inodes";
        } else if section == "bytes" {
            df_bytes_lines.push(line);
        } else if section == "inodes" {
            df_inodes_lines.push(line);
        }
    }

    let (filesystem, total_bytes, used_bytes, avail_bytes, use_percent, mount_point) =
        parse_df_bytes_output(&df_bytes_lines.join("\n")).unwrap_or_default();

    let inodes = parse_df_inodes_output(&df_inodes_lines.join("\n")).unwrap_or_default();

    Ok(DiskMountStat {
        path: target.to_string(),
        filesystem,
        mount_point,
        exists,
        mounted_separately,
        total_bytes,
        used_bytes,
        avail_bytes,
        use_percent,
        inodes_total: inodes.total,
        inodes_used: inodes.used,
        inodes_free: inodes.free,
        inodes_percent: inodes.percent,
    })
}

/// List sorted `du -B1` entries under `target`.
pub async fn inspect_du(
    ssh: &SshSession,
    target: &str,
    max_depth: u32,
    top_n: usize,
) -> Result<Vec<DuEntry>, AppError> {
    let esc = shell_escape(target);
    let cmd = format!(
        "du -B1 --max-depth={max_depth} -- {esc} 2>/dev/null | sort -rn | head -n {top_n}"
    );
    let out = ssh.exec_streaming(&cmd, None, None, true).await?;
    Ok(parse_du_output(&out.stdout))
}

/// Inspect and optionally clean hidden top-level cache/trash items under `/root/autodl-fs`.
pub async fn clean_fs_cache(
    ssh: &SshSession,
    dry_run: bool,
) -> Result<CleanCacheReport, AppError> {
    let stat = inspect_disk_path(ssh, AUTODL_FS_ROOT).await?;
    if !stat.exists || !stat.mounted_separately {
        return Err(AppError::NotFound(format!(
            "File storage is not mounted at '{AUTODL_FS_ROOT}'. Initialize File Storage in the AutoDL console and reboot the instance first."
        )));
    }

    let list_cmd = format!(
        "find {} -mindepth 1 -maxdepth 1 -name '.*' | while IFS= read -r p; do [ -e \"$p\" ] || continue; t='f'; [ -d \"$p\" ] && t='d'; sz=$(du -sb -- \"$p\" 2>/dev/null | awk '{{print $1}}'); printf '%s\\t%s\\t%s\\n' \"$t\" \"${{sz:-0}}\" \"$p\"; done",
        shell_escape(AUTODL_FS_ROOT)
    );
    let out = ssh.exec_streaming(&list_cmd, None, None, true).await?;
    let items = parse_cache_list_output(&out.stdout);
    let reclaimed_bytes: u64 = items.iter().map(|i| i.size_bytes).sum();

    let mut removed_count = 0usize;
    if !dry_run && !items.is_empty() {
        let targets: Vec<String> = items.iter().map(|i| shell_escape(&i.path)).collect();
        let rm_cmd = format!("rm -rf -- {}", targets.join(" "));
        let rm_out = ssh.exec_streaming(&rm_cmd, None, None, true).await?;
        if rm_out.exit_code != 0 {
            return Err(AppError::RemoteExit {
                code: rm_out.exit_code,
                message: format!("Failed cleaning cache in {AUTODL_FS_ROOT}: {}", rm_out.stderr.trim()),
            });
        }
        removed_count = items.len();
    }

    Ok(CleanCacheReport {
        mounted: true,
        dry_run,
        items,
        reclaimed_bytes,
        removed_count,
    })
}
