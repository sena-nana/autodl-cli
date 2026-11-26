use std::path::PathBuf;
use std::time::Duration;

use clap::Args;

use crate::error::AppError;
use crate::output::{format_bytes, make_table};
use crate::ssh::transfer::{
    PackOptions, download_path, remote_rm, sftp_ls, sync_directory, upload_path,
};
use crate::ssh::{SshEndpoint, SshSession};
use crate::state::resolve_session;

use super::CliContext;

#[derive(Debug, Args)]
pub struct UploadArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Local file or directory path
    pub local: PathBuf,
    /// Remote destination path (default: /root/autodl-tmp)
    #[arg(default_value = "/root/autodl-tmp")]
    pub remote: String,
    /// Exclude glob pattern(s) when uploading a directory
    #[arg(long = "exclude")]
    pub excludes: Vec<String>,
    #[arg(long = "no-gitignore")]
    pub no_gitignore: bool,
    #[arg(long = "no-default-excludes")]
    pub no_default_excludes: bool,
}

#[derive(Debug, Args)]
pub struct SyncArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Local project directory to sync (default: .)
    #[arg(default_value = ".")]
    pub local_dir: PathBuf,
    /// Remote directory on the instance (default: /root/autodl-tmp/workspace)
    #[arg(default_value = "/root/autodl-tmp/workspace")]
    pub remote_dir: String,
    /// Exclude glob pattern(s) (repeatable)
    #[arg(long = "exclude")]
    pub excludes: Vec<String>,
    #[arg(long = "no-gitignore")]
    pub no_gitignore: bool,
    #[arg(long = "no-default-excludes")]
    pub no_default_excludes: bool,
    /// Clean the remote destination directory before unpacking synced code
    #[arg(long = "delete")]
    pub delete: bool,
}

#[derive(Debug, Args)]
pub struct DownloadArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Remote file or directory path
    pub remote: String,
    /// Local destination file or directory (default: .)
    #[arg(default_value = ".")]
    pub local: PathBuf,
    /// Recursively download a remote directory (streamed as tar.gz)
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Remote path to list (default: /root/autodl-tmp)
    #[arg(default_value = "/root/autodl-tmp")]
    pub path: String,
}

#[derive(Debug, Args)]
pub struct RmArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Remote path to remove
    pub path: String,
    /// Recursively remove directory (`rm -rf`)
    #[arg(short = 'r', long = "recursive")]
    pub recursive: bool,
}

pub async fn handle_upload(ctx: &CliContext, args: UploadArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;

    let opts = PackOptions {
        use_gitignore: !args.no_gitignore,
        use_default_excludes: !args.no_default_excludes,
        extra_excludes: args.excludes,
    };

    ctx.out.status(&format!(
        "Uploading '{}' -> {}:{}...",
        args.local.display(),
        resolved.name,
        args.remote
    ));
    let stats = upload_path(&ssh, &args.local, &args.remote, &opts).await?;
    ssh.close().await;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "local": args.local.display().to_string(),
            "remote": args.remote,
            "stats": stats,
        }))?;
    } else {
        ctx.out.println(&format!(
            "Uploaded {} file(s) ({}, compressed {}) to {}:{}",
            stats.files,
            format_bytes(stats.bytes_raw),
            format_bytes(stats.bytes_compressed),
            resolved.name,
            args.remote
        ));
    }
    Ok(())
}

pub async fn handle_sync(ctx: &CliContext, args: SyncArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;

    let opts = PackOptions {
        use_gitignore: !args.no_gitignore,
        use_default_excludes: !args.no_default_excludes,
        extra_excludes: args.excludes,
    };

    ctx.out.status(&format!(
        "Syncing '{}' -> {}:{}...",
        args.local_dir.display(),
        resolved.name,
        args.remote_dir
    ));
    let stats = sync_directory(&ssh, &args.local_dir, &args.remote_dir, &opts, args.delete).await?;
    ssh.close().await;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "local_dir": args.local_dir.display().to_string(),
            "remote_dir": args.remote_dir,
            "deleted_before_sync": args.delete,
            "stats": stats,
        }))?;
    } else {
        ctx.out.println(&format!(
            "Synced {} file(s) ({} raw -> {} compressed) to {}:{}",
            stats.files,
            format_bytes(stats.bytes_raw),
            format_bytes(stats.bytes_compressed),
            resolved.name,
            args.remote_dir
        ));
    }
    Ok(())
}

pub async fn handle_download(ctx: &CliContext, args: DownloadArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;

    ctx.out.status(&format!(
        "Downloading {}:{} -> '{}'...",
        resolved.name,
        args.remote,
        args.local.display()
    ));
    let (saved, bytes) = download_path(&ssh, &args.remote, &args.local, args.recursive).await?;
    ssh.close().await;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "remote": args.remote,
            "local": saved.display().to_string(),
            "bytes": bytes,
        }))?;
    } else {
        ctx.out.println(&format!(
            "Downloaded {} ({}) to '{}'",
            args.remote,
            format_bytes(bytes),
            saved.display()
        ));
    }
    Ok(())
}

pub async fn handle_ls(ctx: &CliContext, args: LsArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;
    let sftp = ssh.open_sftp().await?;
    let entries = sftp_ls(&sftp, &args.path).await?;
    let _ = sftp.close().await;
    ssh.close().await;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "path": args.path,
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

pub async fn handle_rm(ctx: &CliContext, args: RmArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;

    remote_rm(&ssh, &args.path, args.recursive).await?;
    ssh.close().await;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "removed": args.path,
            "recursive": args.recursive,
        }))?;
    } else {
        ctx.out.println(&format!("Removed '{}'", args.path));
    }
    Ok(())
}
