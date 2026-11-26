use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Subcommand};

use crate::error::AppError;
use crate::output::{format_bytes, make_table};
use crate::ssh::storage::{
    AUTODL_FS_INODE_LIMIT, AUTODL_FS_ROOT, AUTODL_TMP_ROOT, clean_fs_cache, ensure_not_fs_root,
    inspect_disk_path, inspect_du, resolve_fs_path,
};
use crate::ssh::transfer::{
    PackOptions, download_path, remote_rm, sftp_ls, sync_directory, upload_path,
};
use crate::ssh::{SshEndpoint, SshSession};
use crate::state::{ResolvedSession, resolve_session};

use super::CliContext;

#[derive(Debug, Args)]
pub struct FsArgs {
    #[command(subcommand)]
    pub command: FsCommand,
}

#[derive(Debug, Subcommand)]
pub enum FsCommand {
    /// Inspect `/root/autodl-fs` mount status, capacity, and 200k inode usage
    Status {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
    },
    /// List files in `/root/autodl-fs` (or a subpath)
    Ls {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        /// Relative subpath under `/root/autodl-fs` (default: `/root/autodl-fs`)
        path: Option<String>,
    },
    /// Upload a local file or directory into `/root/autodl-fs`
    Upload {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        local: PathBuf,
        /// Remote path inside `/root/autodl-fs` (default: `/root/autodl-fs`)
        remote: Option<String>,
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        #[arg(long = "no-gitignore")]
        no_gitignore: bool,
        #[arg(long = "no-default-excludes")]
        no_default_excludes: bool,
        /// Allow uploading even if `/root/autodl-fs` is not separately mounted
        #[arg(long = "allow-unmounted")]
        allow_unmounted: bool,
    },
    /// Sync a local project directory into `/root/autodl-fs` (default: `/root/autodl-fs/workspace`)
    Sync {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        #[arg(default_value = ".")]
        local_dir: PathBuf,
        #[arg(default_value = "workspace")]
        remote_dir: String,
        #[arg(long = "exclude")]
        excludes: Vec<String>,
        #[arg(long = "no-gitignore")]
        no_gitignore: bool,
        #[arg(long = "no-default-excludes")]
        no_default_excludes: bool,
        #[arg(long = "delete")]
        delete: bool,
        /// Allow syncing even if `/root/autodl-fs` is not separately mounted
        #[arg(long = "allow-unmounted")]
        allow_unmounted: bool,
    },
    /// Download a file or directory (`-r`) from `/root/autodl-fs`
    Download {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        remote: String,
        #[arg(default_value = ".")]
        local: PathBuf,
        #[arg(short = 'r', long = "recursive")]
        recursive: bool,
    },
    /// Remove a file or directory (`-r`) inside `/root/autodl-fs`
    Rm {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        path: String,
        #[arg(short = 'r', long = "recursive")]
        recursive: bool,
    },
    /// Show disk usage breakdown (`du`) inside `/root/autodl-fs`
    Du {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        path: Option<String>,
        #[arg(long, default_value_t = 1)]
        depth: u32,
        #[arg(long, default_value_t = 20)]
        top: usize,
    },
    /// List or clean hidden cache/trash files in `/root/autodl-fs`
    CleanCache {
        #[arg(short = 's', long = "session")]
        session: Option<String>,
        /// Preview hidden cache files without deleting them
        #[arg(long = "dry-run")]
        dry_run: bool,
        /// Skip interactive confirmation prompt
        #[arg(short = 'y', long = "yes")]
        yes: bool,
    },
}

#[derive(Debug, Args)]
pub struct DiskArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Specific path(s) to inspect (defaults to `/`, `/root/autodl-tmp`, `/root/autodl-fs`)
    #[arg(long = "path")]
    pub paths: Vec<String>,
    /// Also run `du` directory size breakdown on this path (optional value defaults to `/root/autodl-tmp`)
    #[arg(long = "du", num_args = 0..=1, default_missing_value = AUTODL_TMP_ROOT)]
    pub du: Option<String>,
    #[arg(long, default_value_t = 1)]
    pub depth: u32,
    #[arg(long, default_value_t = 15)]
    pub top: usize,
}

async fn connect_session_ssh(
    ctx: &CliContext,
    target: Option<&str>,
) -> Result<(ResolvedSession, SshSession), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(target, &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;
    Ok((resolved, ssh))
}

async fn ensure_fs_mounted(ssh: &SshSession, allow_unmounted: bool) -> Result<(), AppError> {
    if allow_unmounted {
        return Ok(());
    }
    let stat = inspect_disk_path(ssh, AUTODL_FS_ROOT).await?;
    if !stat.exists || !stat.mounted_separately {
        return Err(AppError::NotFound(format!(
            "Shared file storage is not mounted at '{AUTODL_FS_ROOT}'. Initialize File Storage in the AutoDL console and restart the instance (or pass --allow-unmounted)."
        )));
    }
    Ok(())
}

pub async fn handle_fs(ctx: &CliContext, args: FsArgs) -> Result<(), AppError> {
    match args.command {
        FsCommand::Status { session } => {
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            let stat = inspect_disk_path(&ssh, AUTODL_FS_ROOT).await?;
            ssh.close().await;

            let mounted = stat.exists && stat.mounted_separately;
            let inode_pct = stat.inodes_percent.unwrap_or(0.0);
            let inode_used = stat.inodes_used.unwrap_or(0);
            let inode_warning = mounted
                && (inode_pct >= 80.0 || inode_used >= (AUTODL_FS_INODE_LIMIT * 80 / 100));

            if inode_warning {
                ctx.out.warn(&format!(
                    "File storage inode usage is high ({inode_used} used, {inode_pct:.1}%). AutoDL enforces a 200,000 inode limit on /root/autodl-fs."
                ));
            }

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "mounted": mounted,
                    "inode_limit": AUTODL_FS_INODE_LIMIT,
                    "inode_warning": inode_warning,
                    "stat": stat,
                }))?;
                return Ok(());
            }

            let mut table = make_table(&["Property", "Value"]);
            table.add_row(vec!["Session", &resolved.name]);
            table.add_row(vec!["Mount Path", AUTODL_FS_ROOT]);
            table.add_row(vec![
                "Mounted Separately",
                if mounted { "yes" } else { "NO (not initialized or needs reboot)" },
            ]);
            table.add_row(vec!["Filesystem", &stat.filesystem]);
            table.add_row(vec![
                "Capacity",
                &format!(
                    "{} used / {} total ({} avail, {:.1}%)",
                    format_bytes(stat.used_bytes),
                    format_bytes(stat.total_bytes),
                    format_bytes(stat.avail_bytes),
                    stat.use_percent
                ),
            ]);
            let inode_str = match (stat.inodes_used, stat.inodes_total, stat.inodes_percent) {
                (Some(u), Some(t), Some(p)) => {
                    format!("{u} / {t} ({p:.1}%, AutoDL limit {AUTODL_FS_INODE_LIMIT})")
                }
                _ => "n/a".to_string(),
            };
            table.add_row(vec!["Inodes", &inode_str]);
            println!("{table}");
            Ok(())
        }
        FsCommand::Ls { session, path } => {
            let target = resolve_fs_path(path.as_deref())?;
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            let sftp = ssh.open_sftp().await?;
            let entries = sftp_ls(&sftp, &target).await?;
            let _ = sftp.close().await;
            ssh.close().await;

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "path": target,
                    "entries": entries,
                }))?;
                return Ok(());
            }

            if entries.is_empty() {
                ctx.out.println("(empty directory)");
                return Ok(());
            }

            let mut table = make_table(&["Type", "Name", "Size"]);
            for e in entries {
                table.add_row(vec![
                    if e.is_dir { "dir" } else { "file" },
                    &e.name,
                    &if e.is_dir {
                        "-".to_string()
                    } else {
                        format_bytes(e.size)
                    },
                ]);
            }
            println!("{table}");
            Ok(())
        }
        FsCommand::Upload {
            session,
            local,
            remote,
            excludes,
            no_gitignore,
            no_default_excludes,
            allow_unmounted,
        } => {
            let target = resolve_fs_path(remote.as_deref())?;
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            ensure_fs_mounted(&ssh, allow_unmounted).await?;

            let opts = PackOptions {
                use_gitignore: !no_gitignore,
                use_default_excludes: !no_default_excludes,
                extra_excludes: excludes,
            };
            ctx.out.status(&format!(
                "Uploading '{}' -> {}:{}...",
                local.display(),
                resolved.name,
                target
            ));
            let stats = upload_path(&ssh, &local, &target, &opts).await?;
            ssh.close().await;

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "local": local.display().to_string(),
                    "remote": target,
                    "stats": stats,
                }))?;
            } else {
                ctx.out.println(&format!(
                    "Uploaded {} file(s) ({} raw -> {} compressed) to {}:{}",
                    stats.files,
                    format_bytes(stats.bytes_raw),
                    format_bytes(stats.bytes_compressed),
                    resolved.name,
                    target
                ));
            }
            Ok(())
        }
        FsCommand::Sync {
            session,
            local_dir,
            remote_dir,
            excludes,
            no_gitignore,
            no_default_excludes,
            delete,
            allow_unmounted,
        } => {
            let target = resolve_fs_path(Some(&remote_dir))?;
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            ensure_fs_mounted(&ssh, allow_unmounted).await?;

            let opts = PackOptions {
                use_gitignore: !no_gitignore,
                use_default_excludes: !no_default_excludes,
                extra_excludes: excludes,
            };
            ctx.out.status(&format!(
                "Syncing '{}' -> {}:{}...",
                local_dir.display(),
                resolved.name,
                target
            ));
            let stats = sync_directory(&ssh, &local_dir, &target, &opts, delete).await?;
            ssh.close().await;

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "local_dir": local_dir.display().to_string(),
                    "remote_dir": target,
                    "deleted_before_sync": delete,
                    "stats": stats,
                }))?;
            } else {
                ctx.out.println(&format!(
                    "Synced {} file(s) ({} raw -> {} compressed) to {}:{}",
                    stats.files,
                    format_bytes(stats.bytes_raw),
                    format_bytes(stats.bytes_compressed),
                    resolved.name,
                    target
                ));
            }
            Ok(())
        }
        FsCommand::Download {
            session,
            remote,
            local,
            recursive,
        } => {
            let target = resolve_fs_path(Some(&remote))?;
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            ctx.out.status(&format!(
                "Downloading {}:{} -> '{}'...",
                resolved.name,
                target,
                local.display()
            ));
            let (saved, bytes) = download_path(&ssh, &target, &local, recursive).await?;
            ssh.close().await;

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "remote": target,
                    "local": saved.display().to_string(),
                    "bytes": bytes,
                }))?;
            } else {
                ctx.out.println(&format!(
                    "Downloaded {} ({}) to '{}'",
                    target,
                    format_bytes(bytes),
                    saved.display()
                ));
            }
            Ok(())
        }
        FsCommand::Rm {
            session,
            path,
            recursive,
        } => {
            let target = resolve_fs_path(Some(&path))?;
            ensure_not_fs_root(&target)?;
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            remote_rm(&ssh, &target, recursive).await?;
            ssh.close().await;

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "removed": target,
                    "recursive": recursive,
                }))?;
            } else {
                ctx.out.println(&format!("Removed '{target}'"));
            }
            Ok(())
        }
        FsCommand::Du {
            session,
            path,
            depth,
            top,
        } => {
            let target = resolve_fs_path(path.as_deref())?;
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            let entries = inspect_du(&ssh, &target, depth, top).await?;
            ssh.close().await;

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "target": target,
                    "entries": entries,
                }))?;
                return Ok(());
            }

            if entries.is_empty() {
                ctx.out.println("(no entries)");
                return Ok(());
            }
            let mut table = make_table(&["Size", "Path"]);
            for e in entries {
                table.add_row(vec![&format_bytes(e.bytes), &e.path]);
            }
            println!("{table}");
            Ok(())
        }
        FsCommand::CleanCache {
            session,
            dry_run,
            yes,
        } => {
            let (resolved, ssh) = connect_session_ssh(ctx, session.as_deref()).await?;
            if !dry_run && !yes && !ctx.out.json && io::stdin().is_terminal() {
                eprint!(
                    "Remove all hidden cache/trash files (.*) in {AUTODL_FS_ROOT} on '{}'? [y/N]: ",
                    resolved.name
                );
                let _ = io::stderr().flush();
                let mut line = String::new();
                io::stdin().read_line(&mut line)?;
                let ans = line.trim().to_ascii_lowercase();
                if ans != "y" && ans != "yes" {
                    ssh.close().await;
                    ctx.out.println("Cancelled.");
                    return Ok(());
                }
            }

            let report = clean_fs_cache(&ssh, dry_run).await?;
            ssh.close().await;

            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "session": resolved.name,
                    "instance_uuid": resolved.instance_uuid,
                    "report": report,
                }))?;
                return Ok(());
            }

            if report.items.is_empty() {
                ctx.out.println("No hidden cache files found in /root/autodl-fs.");
                return Ok(());
            }
            let mut table = make_table(&["Type", "Size", "Path"]);
            for item in &report.items {
                table.add_row(vec![
                    if item.is_dir { "dir" } else { "file" },
                    &format_bytes(item.size_bytes),
                    &item.path,
                ]);
            }
            println!("{table}");
            if dry_run {
                ctx.out.println(&format!(
                    "Dry run: {} item(s), {} reclaimable.",
                    report.items.len(),
                    format_bytes(report.reclaimed_bytes)
                ));
            } else {
                ctx.out.println(&format!(
                    "Cleaned {} item(s), reclaimed {}.",
                    report.removed_count,
                    format_bytes(report.reclaimed_bytes)
                ));
            }
            Ok(())
        }
    }
}

pub async fn handle_disk(ctx: &CliContext, args: DiskArgs) -> Result<(), AppError> {
    let targets = if args.paths.is_empty() {
        vec![
            "/".to_string(),
            AUTODL_TMP_ROOT.to_string(),
            AUTODL_FS_ROOT.to_string(),
        ]
    } else {
        args.paths.clone()
    };

    let (resolved, ssh) = connect_session_ssh(ctx, args.session.as_deref()).await?;
    let mut mounts = Vec::with_capacity(targets.len());
    for t in &targets {
        mounts.push(inspect_disk_path(&ssh, t).await?);
    }

    let du_entries = if let Some(du_path) = &args.du {
        Some((
            du_path.clone(),
            inspect_du(&ssh, du_path, args.depth, args.top).await?,
        ))
    } else {
        None
    };

    ssh.close().await;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "mounts": mounts,
            "du": du_entries.as_ref().map(|(p, entries)| serde_json::json!({
                "path": p,
                "entries": entries,
            })),
        }))?;
        return Ok(());
    }

    let mut table = make_table(&[
        "Path",
        "Mounted",
        "Used / Total",
        "Avail",
        "Use%",
        "Inodes (Used / Total)",
    ]);
    for m in &mounts {
        let mounted_str = if !m.exists {
            "missing"
        } else if m.path == "/" || m.mounted_separately {
            "yes"
        } else {
            "no (same fs)"
        };
        let cap_str = if m.exists && m.total_bytes > 0 {
            format!(
                "{} / {}",
                format_bytes(m.used_bytes),
                format_bytes(m.total_bytes)
            )
        } else {
            "-".to_string()
        };
        let avail_str = if m.exists && m.total_bytes > 0 {
            format_bytes(m.avail_bytes)
        } else {
            "-".to_string()
        };
        let pct_str = if m.exists && m.total_bytes > 0 {
            format!("{:.1}%", m.use_percent)
        } else {
            "-".to_string()
        };
        let inode_str = match (m.inodes_used, m.inodes_total, m.inodes_percent) {
            (Some(u), Some(t), Some(p)) => format!("{u} / {t} ({p:.1}%)"),
            _ => "-".to_string(),
        };
        table.add_row(vec![
            &m.path,
            mounted_str,
            &cap_str,
            &avail_str,
            &pct_str,
            &inode_str,
        ]);
    }
    println!("{table}");

    if let Some((du_path, entries)) = du_entries {
        println!("\nTop disk usage under {du_path}:");
        let mut du_table = make_table(&["Size", "Path"]);
        for e in entries {
            du_table.add_row(vec![&format_bytes(e.bytes), &e.path]);
        }
        println!("{du_table}");
    }
    Ok(())
}
