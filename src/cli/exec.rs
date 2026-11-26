use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::time::Duration;

use clap::Args;

use crate::catalog::parse_cuda_version;
use crate::error::AppError;
use crate::ssh::transfer::{PackOptions, download_path, shell_escape, upload_file_sftp};
use crate::ssh::{SshEndpoint, SshSession};
use crate::state::resolve_session;
use crate::workflow::run::{DownloadArtifact, EphemeralRunOptions, execute_ephemeral_run};
use crate::workflow::{
    build_install_command, build_remote_exec_command, build_script_run_command,
    parse_download_spec, parse_env_pair,
};

use super::CliContext;

#[derive(Debug, Args)]
pub struct ExecArgs {
    /// Target session name or instance UUID (pro-...). Auto-selected if only one session is active.
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Local script file (.py, .sh, etc.) to upload and execute remotely
    #[arg(short = 'f', long = "file")]
    pub file: Option<PathBuf>,
    /// Shell command string to execute remotely
    #[arg(long = "cmd")]
    pub cmd: Option<String>,
    /// Interpret piped stdin explicitly as Python (`python3 -u -`) instead of auto-detecting
    #[arg(long = "python")]
    pub python: bool,
    /// Upload a local file into the remote workdir before running (repeatable)
    #[arg(long = "upload")]
    pub upload: Vec<PathBuf>,
    /// Download artifact after execution (`REMOTE:LOCAL`, repeatable)
    #[arg(long = "download")]
    pub download: Vec<String>,
    /// Remote working directory (default: /root/autodl-tmp)
    #[arg(long = "workdir")]
    pub workdir: Option<String>,
    /// Environment variable (`KEY=VALUE`, repeatable)
    #[arg(long = "env")]
    pub env: Vec<String>,
    /// Idle timeout in seconds with no output (`0` = no limit)
    #[arg(long = "timeout", default_value_t = 300)]
    pub timeout_secs: u64,
    /// Arguments forwarded to the executed script (`-- arg1 arg2`)
    #[arg(last = true)]
    pub script_args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct RunArgs {
    /// Local script (.py or .sh) to run on a fresh GPU instance
    pub script: Option<PathBuf>,
    /// Shell command to execute instead of a local script file
    #[arg(long = "cmd")]
    pub cmd: Option<String>,
    /// Name for the ephemeral session (default: run-<hex>)
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// GPU model or spec UUID (e.g., 4090, 4090D, 5090, H800, PRO6000)
    #[arg(long)]
    pub gpu: Option<String>,
    /// Number of GPUs (1..=4)
    #[arg(long = "gpu-count", default_value_t = 1)]
    pub gpu_count: u32,
    /// Image UUID or alias (e.g., pytorch2.0, miniconda, image-...)
    #[arg(long)]
    pub image: Option<String>,
    /// Minimum host CUDA version (e.g., 11.8, 12.1)
    #[arg(long)]
    pub cuda: Option<String>,
    /// Preferred region(s) (repeatable or comma-separated)
    #[arg(long = "region", value_delimiter = ',')]
    pub regions: Vec<String>,
    /// Expand system disk in GB (0..=500)
    #[arg(long = "disk", default_value_t = 0)]
    pub expand_disk_gb: u32,
    /// Sync a local project directory into the instance before running
    #[arg(long = "sync")]
    pub sync_dir: Option<PathBuf>,
    /// Remote destination for --sync (default: /root/autodl-tmp/workspace)
    #[arg(long = "sync-dest", default_value = "/root/autodl-tmp/workspace")]
    pub sync_dest: String,
    /// Exclude glob pattern(s) during --sync (repeatable)
    #[arg(long = "exclude")]
    pub excludes: Vec<String>,
    /// Do not respect .gitignore during --sync
    #[arg(long = "no-gitignore")]
    pub no_gitignore: bool,
    /// Do not apply built-in dev/ML excludes (.git, target, __pycache__, ...) during --sync
    #[arg(long = "no-default-excludes")]
    pub no_default_excludes: bool,
    /// Python packages to install before executing (comma-separated or repeatable)
    #[arg(long = "install", value_delimiter = ',')]
    pub install_packages: Vec<String>,
    /// Local requirements.txt to install before executing
    #[arg(short = 'r', long = "requirements")]
    pub requirements: Option<PathBuf>,
    /// Download remote artifact after job (`REMOTE:LOCAL`, repeatable)
    #[arg(long = "download")]
    pub download: Vec<String>,
    /// Download artifacts even if the remote job exits non-zero
    #[arg(long = "download-on-error")]
    pub download_on_error: bool,
    /// Environment variable (`KEY=VALUE`, repeatable)
    #[arg(long = "env")]
    pub env: Vec<String>,
    /// Remote working directory
    #[arg(long = "workdir")]
    pub workdir: Option<String>,
    /// Max seconds waiting for instance boot (default: 300)
    #[arg(long = "wait-timeout", default_value_t = 300)]
    pub wait_timeout_secs: u64,
    /// Idle timeout in seconds with no output (`0` = no limit)
    #[arg(long = "timeout", default_value_t = 600)]
    pub idle_timeout_secs: u64,
    /// Keep instance running after job finishes (do not release)
    #[arg(long = "keep")]
    pub keep: bool,
    /// Power off instead of releasing after job finishes
    #[arg(long = "power-off")]
    pub power_off: bool,
    /// Save an image before releasing the instance
    #[arg(long = "save-image")]
    pub save_image: Option<String>,
    /// Optional boot start command attached at creation time
    #[arg(long = "start-cmd")]
    pub start_cmd: Option<String>,
    /// Arguments forwarded to SCRIPT (`-- arg1 arg2`)
    #[arg(last = true)]
    pub script_args: Vec<String>,
}

#[derive(Debug, Args)]
pub struct SshArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Print SSH connection details (host, port, user, command) without connecting
    #[arg(long = "print-cmd")]
    pub print_cmd: bool,
    /// Optional one-shot remote command (`autodl ssh -- nvidia-smi`)
    #[arg(last = true)]
    pub cmd: Vec<String>,
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Local requirements.txt file to upload and install
    #[arg(short = 'r', long = "requirements")]
    pub requirements: Option<PathBuf>,
    /// Pass --upgrade to uv/pip
    #[arg(short = 'U', long = "upgrade")]
    pub upgrade: bool,
    /// Package names to install
    pub packages: Vec<String>,
}

pub async fn handle_exec(ctx: &CliContext, args: ExecArgs) -> Result<(), AppError> {
    if args.file.is_some() && args.cmd.is_some() {
        return Err(AppError::Validation(
            "Pass either -f/--file or --cmd, not both".into(),
        ));
    }

    let mut envs = Vec::with_capacity(args.env.len());
    for pair in &args.env {
        envs.push(parse_env_pair(pair)?);
    }
    let mut downloads = Vec::with_capacity(args.download.len());
    for spec in &args.download {
        downloads.push(parse_download_spec(spec)?);
    }

    let piped_stdin = if args.file.is_none() && args.cmd.is_none() {
        if io::stdin().is_terminal() {
            return Err(AppError::Validation(
                "Pass -f <FILE>, --cmd <COMMAND>, or pipe code/commands via stdin".into(),
            ));
        }
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        if buf.trim().is_empty() {
            return Err(AppError::Validation("Piped stdin is empty".into()));
        }
        Some(buf)
    } else {
        None
    };

    if let Some(f) = &args.file
        && !f.is_file()
    {
        return Err(AppError::NotFound(format!(
            "Local script '{}' not found",
            f.display()
        )));
    }
    for up in &args.upload {
        if !up.is_file() {
            return Err(AppError::NotFound(format!(
                "Upload file '{}' not found",
                up.display()
            )));
        }
    }

    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;

    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;
    let workdir = args
        .workdir
        .clone()
        .unwrap_or_else(|| resolved.default_remote_dir.clone());

    if !args.upload.is_empty() || args.file.is_some() {
        let sftp = ssh.open_sftp().await?;
        for up in &args.upload {
            let fname = up
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| AppError::Validation("Invalid upload filename".into()))?;
            let rpath = format!("{}/{}", workdir.trim_end_matches('/'), fname);
            upload_file_sftp(&sftp, up, &rpath).await?;
        }
        if let Some(file_path) = &args.file {
            let fname = file_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("exec_script.py");
            let remote_script = format!("{}/{}", workdir.trim_end_matches('/'), fname);
            upload_file_sftp(&sftp, file_path, &remote_script).await?;
        }
        let _ = sftp.close().await;
    }

    let (remote_cmd, stdin_bytes) = if let Some(file_path) = &args.file {
        let fname = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("exec_script.py");
        let remote_script = format!("{}/{}", workdir.trim_end_matches('/'), fname);
        (
            build_script_run_command(&remote_script, &args.script_args),
            None,
        )
    } else if let Some(cmd) = &args.cmd {
        (cmd.clone(), None)
    } else if let Some(stdin_code) = piped_stdin {
        let runner = if args.python || looks_like_python(&stdin_code) {
            "python3 -u -"
        } else {
            "bash -s"
        };
        (runner.to_string(), Some(stdin_code.into_bytes()))
    } else {
        unreachable!("validated above")
    };

    let full_cmd = build_remote_exec_command(Some(&workdir), &envs, &remote_cmd);
    let idle_limit = if args.timeout_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(args.timeout_secs))
    };

    let exec_out = ssh
        .exec_streaming(&full_cmd, stdin_bytes, idle_limit, ctx.out.json)
        .await?;

    let mut downloaded = Vec::new();
    if exec_out.exit_code == 0 {
        for (remote_src, local_dst) in &downloads {
            let resolved_remote = if remote_src.starts_with('/') {
                remote_src.clone()
            } else {
                format!(
                    "{}/{}",
                    workdir.trim_end_matches('/'),
                    remote_src.trim_start_matches("./")
                )
            };
            let (saved, bytes) = download_path(&ssh, &resolved_remote, local_dst, true).await?;
            downloaded.push(DownloadArtifact {
                remote: resolved_remote,
                local: saved.display().to_string(),
                bytes,
            });
        }
    }

    ssh.close().await;

    let ok = exec_out.exit_code == 0;
    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": ok,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "exit_code": exec_out.exit_code,
            "stdout": exec_out.stdout,
            "stderr": exec_out.stderr,
            "downloads": downloaded,
        }))?;
        if !ok {
            return Err(AppError::ReportedExit(exec_out.exit_code));
        }
    } else if !ok {
        return Err(AppError::RemoteExit {
            code: exec_out.exit_code,
            message: format!("Remote command exited with status {}", exec_out.exit_code),
        });
    }
    Ok(())
}

pub async fn handle_run(ctx: &CliContext, args: RunArgs) -> Result<(), AppError> {
    if !(1..=4).contains(&args.gpu_count) {
        return Err(AppError::Validation(
            "--gpu-count must be between 1 and 4".into(),
        ));
    }
    if args.expand_disk_gb > 500 {
        return Err(AppError::Validation(
            "--disk must be between 0 and 500 GB".into(),
        ));
    }
    if args.keep && args.power_off {
        return Err(AppError::Validation(
            "Pass either --keep or --power-off, not both".into(),
        ));
    }

    let (_, profile) = ctx.api_client()?;
    let (client, _) = ctx.api_client()?;
    let store = ctx.session_store();

    let cuda_version = match args.cuda.as_deref() {
        Some(c) => Some(parse_cuda_version(c)?),
        None => None,
    };

    let mut envs = Vec::with_capacity(args.env.len());
    for pair in &args.env {
        envs.push(parse_env_pair(pair)?);
    }
    let mut downloads = Vec::with_capacity(args.download.len());
    for spec in &args.download {
        downloads.push(parse_download_spec(spec)?);
    }

    let opts = EphemeralRunOptions {
        profile,
        session_name: args.session,
        gpu: args.gpu,
        gpu_count: args.gpu_count,
        image: args.image,
        cuda_version,
        regions: args.regions,
        expand_disk_gb: args.expand_disk_gb,
        script: args.script,
        script_args: args.script_args,
        cmd: args.cmd,
        sync_dir: args.sync_dir,
        sync_dest: args.sync_dest,
        pack_options: PackOptions {
            use_gitignore: !args.no_gitignore,
            use_default_excludes: !args.no_default_excludes,
            extra_excludes: args.excludes,
        },
        install_packages: args.install_packages,
        requirements_file: args.requirements,
        downloads,
        download_on_error: args.download_on_error,
        envs,
        workdir: args.workdir,
        wait_timeout_secs: args.wait_timeout_secs,
        idle_timeout_secs: args.idle_timeout_secs,
        keep: args.keep,
        power_off_only: args.power_off,
        save_image: args.save_image,
        start_cmd: args.start_cmd,
    };

    let report = execute_ephemeral_run(&client, &store, opts, &ctx.out).await?;
    if !ctx.out.json {
        ctx.out.info(&format!(
            "Run finished in {:.1}s (exit_code={}, released={}, kept={})",
            report.duration_secs, report.exit_code, report.released, report.kept
        ));
    }
    Ok(())
}

pub async fn handle_ssh(ctx: &CliContext, args: SshArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;

    if args.print_cmd {
        if ctx.out.json {
            ctx.out.emit_json(&serde_json::json!({
                "ok": true,
                "session": resolved.name,
                "instance_uuid": resolved.instance_uuid,
                "host": endpoint.host,
                "port": endpoint.port,
                "user": endpoint.user,
                "ssh_command": endpoint.ssh_command_display(),
            }))?;
        } else {
            ctx.out.println(&endpoint.ssh_command_display());
        }
        return Ok(());
    }

    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;
    if !args.cmd.is_empty() {
        let joined = if args.cmd.len() == 1 {
            args.cmd[0].clone()
        } else {
            args.cmd
                .iter()
                .map(|a| shell_escape(a))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let out = ssh.exec_streaming(&joined, None, None, ctx.out.json).await?;
        ssh.close().await;
        if ctx.out.json {
            ctx.out.emit_json(&serde_json::json!({
                "ok": out.exit_code == 0,
                "session": resolved.name,
                "instance_uuid": resolved.instance_uuid,
                "exit_code": out.exit_code,
                "stdout": out.stdout,
                "stderr": out.stderr,
            }))?;
            if out.exit_code != 0 {
                return Err(AppError::ReportedExit(out.exit_code));
            }
        } else if out.exit_code != 0 {
            return Err(AppError::RemoteExit {
                code: out.exit_code,
                message: format!("SSH command exited with status {}", out.exit_code),
            });
        }
        return Ok(());
    }

    if ctx.out.json {
        ssh.close().await;
        return Err(AppError::Validation(
            "Interactive SSH shell cannot run with --json; pass `-- <CMD>` or `--print-cmd`"
                .into(),
        ));
    }

    let code = ssh.open_interactive_shell().await?;
    ssh.close().await;
    if code != 0 {
        return Err(AppError::RemoteExit {
            code,
            message: format!("SSH shell exited with status {code}"),
        });
    }
    Ok(())
}

pub async fn handle_install(ctx: &CliContext, args: InstallArgs) -> Result<(), AppError> {
    if args.packages.is_empty() && args.requirements.is_none() {
        return Err(AppError::Validation(
            "Provide package name(s) or -r/--requirements <FILE>".into(),
        ));
    }
    if let Some(req) = &args.requirements
        && !req.is_file()
    {
        return Err(AppError::NotFound(format!(
            "Requirements file '{}' not found",
            req.display()
        )));
    }

    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(60)).await?;

    let remote_req = if let Some(req) = &args.requirements {
        let sftp = ssh.open_sftp().await?;
        let rpath = "/tmp/autodl_requirements.txt".to_string();
        upload_file_sftp(&sftp, req, &rpath).await?;
        let _ = sftp.close().await;
        Some(rpath)
    } else {
        None
    };

    let cmd = build_install_command(&args.packages, remote_req.as_deref(), args.upgrade)?;
    ctx.out.status(&format!(
        "Installing packages on {} ({})...",
        resolved.name, resolved.instance_uuid
    ));
    let out = ssh
        .exec_streaming(&cmd, None, Some(Duration::from_secs(600)), ctx.out.json)
        .await?;
    ssh.close().await;

    let ok = out.exit_code == 0;
    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": ok,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "exit_code": out.exit_code,
            "stdout": out.stdout,
            "stderr": out.stderr,
        }))?;
        if !ok {
            return Err(AppError::ReportedExit(out.exit_code));
        }
    } else if !ok {
        return Err(AppError::RemoteExit {
            code: out.exit_code,
            message: format!("Package installation failed with exit {}", out.exit_code),
        });
    }
    Ok(())
}

fn looks_like_python(code: &str) -> bool {
    let trimmed = code.trim_start();
    trimmed.starts_with("import ")
        || trimmed.starts_with("from ")
        || trimmed.starts_with("print(")
        || trimmed.starts_with("def ")
        || trimmed.starts_with("class ")
}
