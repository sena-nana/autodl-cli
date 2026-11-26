use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use clap::Args;

use crate::api::types::CreateProInstanceReq;
use crate::catalog::{
    DEFAULT_CUDA_VERSION, DEFAULT_GPU_SPEC, DEFAULT_IMAGE_UUID, parse_cuda_version,
    resolve_gpu_spec, resolve_image_uuid,
};
use crate::error::AppError;
use crate::output::{format_bytes, format_cny_milli, make_table};
use crate::ssh::{SshEndpoint, SshSession};
use crate::state::{DEFAULT_REMOTE_DIR, SessionRecord, generate_session_name, resolve_session};
use crate::workflow::{START_EXIT_PATH, START_LOG_PATH, build_start_command};

use super::CliContext;

#[derive(Debug, Args)]
pub struct NewArgs {
    /// Session name (auto-generated if omitted)
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// GPU model or spec UUID (e.g., 4090, 4090D, 5090, H800, PRO6000, v-48g)
    #[arg(long)]
    pub gpu: Option<String>,
    /// Number of GPUs (1..=4)
    #[arg(long = "gpu-count", default_value_t = 1)]
    pub gpu_count: u32,
    /// Image UUID or friendly alias (e.g., pytorch2.0, miniconda, tf2.9, image-...)
    #[arg(long)]
    pub image: Option<String>,
    /// Minimum host driver CUDA version (e.g., 11.8, 12.1, or 118)
    #[arg(long)]
    pub cuda: Option<String>,
    /// Preferred data center region(s) (repeatable or comma-separated, e.g., westDC3,beijingDC2)
    #[arg(long = "region", value_delimiter = ',')]
    pub regions: Vec<String>,
    /// Expand system disk size in GB (0..=500)
    #[arg(long = "disk", default_value_t = 0)]
    pub expand_disk_gb: u32,
    /// Command executed automatically after instance boot
    #[arg(long = "start-cmd")]
    pub start_cmd: Option<String>,
    /// Local script (.sh or .py) encoded & executed automatically after boot
    #[arg(long = "start-script")]
    pub start_script: Option<PathBuf>,
    /// Automatically shut down the instance when the boot start command finishes
    #[arg(long = "auto-shutdown-on-exit")]
    pub auto_shutdown_on_exit: bool,
    /// Pass --start-cmd verbatim without wrapping log capture
    #[arg(long = "raw-start-cmd")]
    pub raw_start_cmd: bool,
    /// Do not wait for instance to reach 'running' state
    #[arg(long = "no-wait")]
    pub no_wait: bool,
    /// Max seconds to wait for instance to reach 'running' state
    #[arg(long = "wait-timeout", default_value_t = 300)]
    pub wait_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub struct SessionTargetArgs {
    /// Target session name or instance UUID (pro-...). Auto-selected if only one session is active.
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
}

#[derive(Debug, Args)]
pub struct PowerOnArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Override boot command executed after power-on
    #[arg(long = "start-cmd")]
    pub start_cmd: Option<String>,
    /// Local script (.sh or .py) encoded & executed after power-on
    #[arg(long = "start-script")]
    pub start_script: Option<PathBuf>,
    /// Automatically shut down when the boot start command exits
    #[arg(long = "auto-shutdown-on-exit")]
    pub auto_shutdown_on_exit: bool,
    /// Pass --start-cmd verbatim without log wrapper
    #[arg(long = "raw-start-cmd")]
    pub raw_start_cmd: bool,
    #[arg(long = "no-wait")]
    pub no_wait: bool,
    #[arg(long = "wait-timeout", default_value_t = 300)]
    pub wait_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub struct PowerOffArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    #[arg(long = "no-wait")]
    pub no_wait: bool,
    #[arg(long = "wait-timeout", default_value_t = 120)]
    pub wait_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub struct StopArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Skip interactive confirmation prompt
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
    #[arg(long = "wait-timeout", default_value_t = 120)]
    pub wait_timeout_secs: u64,
}

#[derive(Debug, Args)]
pub struct UrlArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Open JupyterLab URL in default browser
    #[arg(long)]
    pub open: bool,
}

#[derive(Debug, Args)]
pub struct BootLogArgs {
    #[arg(short = 's', long = "session")]
    pub session: Option<String>,
    /// Number of trailing lines to show (default: all)
    #[arg(short = 'n', long)]
    pub tail: Option<usize>,
}

pub async fn handle_new(ctx: &CliContext, args: NewArgs) -> Result<(), AppError> {
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

    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let session_name = args
        .session
        .clone()
        .unwrap_or_else(|| generate_session_name("s"));

    let (gpu_spec, known_gpu) = match args.gpu.as_deref() {
        Some(g) => resolve_gpu_spec(g),
        None => (DEFAULT_GPU_SPEC.to_string(), true),
    };
    if !known_gpu {
        ctx.out.warn(&format!(
            "GPU spec '{gpu_spec}' is not in the built-in catalog; passing through raw"
        ));
    }

    let (image_uuid, suggested_cuda) = match args.image.as_deref() {
        Some(img) => resolve_image_uuid(img),
        None => (DEFAULT_IMAGE_UUID.to_string(), Some(DEFAULT_CUDA_VERSION)),
    };
    let cuda_v_from = match args.cuda.as_deref() {
        Some(c) => parse_cuda_version(c)?,
        None => suggested_cuda.unwrap_or(DEFAULT_CUDA_VERSION),
    };

    let start_command = build_start_command(
        args.start_cmd.as_deref(),
        args.start_script.as_deref(),
        args.auto_shutdown_on_exit,
        args.raw_start_cmd,
    )?;

    let req = CreateProInstanceReq {
        data_center_list: if args.regions.is_empty() {
            None
        } else {
            Some(args.regions.clone())
        },
        req_gpu_amount: args.gpu_count,
        expand_system_disk_by_gb: args.expand_disk_gb,
        gpu_spec_uuid: gpu_spec.clone(),
        image_uuid: image_uuid.clone(),
        cuda_v_from,
        instance_name: Some(session_name.clone()),
        start_command: start_command.clone(),
    };

    ctx.out.status(&format!(
        "Creating AutoDL instance '{session_name}' ({gpu_spec} x{}, image {image_uuid})...",
        args.gpu_count
    ));
    let instance_uuid = client.create_pro_instance(&req).await?;
    ctx.out.status(&format!(
        "Created instance {instance_uuid} (session '{session_name}')"
    ));

    store.upsert(SessionRecord {
        name: session_name.clone(),
        instance_uuid: instance_uuid.clone(),
        profile: profile.clone(),
        gpu_spec: gpu_spec.clone(),
        gpu_count: args.gpu_count,
        image_uuid: image_uuid.clone(),
        region: args.regions.first().cloned(),
        created_at: Utc::now(),
        default_remote_dir: DEFAULT_REMOTE_DIR.to_string(),
    })?;

    let mut final_status = "creating".to_string();
    let mut snapshot = None;
    if !args.no_wait {
        ctx.out
            .status(&format!("Waiting for {instance_uuid} to start..."));
        final_status = client
            .wait_for_pro_status(
                &instance_uuid,
                &["running"],
                Duration::from_secs(args.wait_timeout_secs),
                Duration::from_secs(3),
            )
            .await?;
        snapshot = client.get_pro_snapshot(&instance_uuid).await.ok();
    }

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": session_name,
            "instance_uuid": instance_uuid,
            "status": final_status,
            "gpu_spec": gpu_spec,
            "gpu_count": args.gpu_count,
            "image_uuid": image_uuid,
            "cuda_v_from": cuda_v_from,
            "has_start_command": start_command.is_some(),
            "ssh_command": snapshot.as_ref().map(|s| s.ssh_command.clone()),
            "jupyter_domain": snapshot.as_ref().map(|s| s.jupyter_domain.clone()),
        }))?;
    } else {
        ctx.out.println(&format!(
            "Session '{session_name}' ready: {instance_uuid} (status: {final_status})"
        ));
        if let Some(snap) = snapshot
            && !snap.ssh_command.is_empty()
        {
            ctx.out.println(&format!("SSH: {}", snap.ssh_command));
        }
    }
    Ok(())
}

pub async fn handle_sessions(ctx: &CliContext) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let local = store.list_for_profile(&profile)?;
    let remote = client.list_all_pro_instances().await?;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "profile": profile,
            "local_sessions": local,
            "remote_instances": remote,
        }))?;
        return Ok(());
    }

    if remote.is_empty() && local.is_empty() {
        ctx.out
            .println("No sessions or Pro instances found. Run `autodl new` to create one.");
        return Ok(());
    }

    let mut table = make_table(&[
        "Session",
        "Instance UUID",
        "Status",
        "GPU",
        "Region",
        "Charge",
        "Created",
    ]);

    for inst in &remote {
        let local_name = local
            .iter()
            .find(|l| l.instance_uuid == inst.uuid)
            .map(|l| l.name.as_str());
        let display_name = local_name.unwrap_or(if inst.name.is_empty() {
            "-"
        } else {
            inst.name.as_str()
        });
        let gpu_label = format!("{} x{}", inst.gpu_spec_uuid, inst.req_gpu_amount);
        table.add_row(vec![
            display_name,
            &inst.uuid,
            &inst.status,
            &gpu_label,
            &inst.region_sign,
            &inst.charge_type,
            &inst.created_at,
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn handle_status(ctx: &CliContext, args: SessionTargetArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let status = client.get_pro_status(&resolved.instance_uuid).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "status": status,
            "snapshot": {
                "region_sign": snap.region_sign,
                "gpu": snap.snapshot_gpu_alias_name,
                "payg_price_milli": snap.payg_price,
                "origin_pay_price_milli": snap.origin_pay_price,
                "cpu_arch": snap.cpu_arch,
                "ssh_command": snap.ssh_command,
                "proxy_host": snap.proxy_host,
                "ssh_port": snap.ssh_port,
                "jupyter_domain": snap.jupyter_domain,
                "service_6006_domain": snap.service_6006_domain,
                "service_6008_domain": snap.service_6008_domain,
                "usage_info": snap.usage_info,
            }
        }))?;
        return Ok(());
    }

    let mut table = make_table(&["Field", "Value"]);
    table.add_row(vec!["Session", &resolved.name]);
    table.add_row(vec!["Instance UUID", &resolved.instance_uuid]);
    table.add_row(vec!["Status", &status]);
    table.add_row(vec!["Region", &snap.region_sign]);
    table.add_row(vec!["GPU", &snap.snapshot_gpu_alias_name]);
    table.add_row(vec![
        "Price (PAYG/hr)",
        &format!(
            "{} (orig {})",
            format_cny_milli(snap.payg_price),
            format_cny_milli(snap.origin_pay_price)
        ),
    ]);
    table.add_row(vec!["SSH", &snap.ssh_command]);
    if !snap.jupyter_domain.is_empty() {
        table.add_row(vec![
            "JupyterLab",
            &format!("https://{}/?token={}", snap.jupyter_domain, snap.jupyter_token),
        ]);
    }
    if let Some(u) = &snap.usage_info {
        table.add_row(vec![
            "CPU / Mem",
            &format!(
                "{:.1}% CPU | {:.1}% Mem ({} / {})",
                u.cpu_usage_percent,
                u.mem_usage_percent,
                format_bytes(u.mem_usage),
                format_bytes(u.mem_limit)
            ),
        ]);
        table.add_row(vec![
            "Root FS",
            &format!(
                "{} / {}",
                format_bytes(u.root_fs_used_size),
                format_bytes(u.root_fs_total_size)
            ),
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn handle_power_on(ctx: &CliContext, args: PowerOnArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;

    let start_command = build_start_command(
        args.start_cmd.as_deref(),
        args.start_script.as_deref(),
        args.auto_shutdown_on_exit,
        args.raw_start_cmd,
    )?;

    client
        .power_on_pro(&resolved.instance_uuid, start_command.clone())
        .await?;

    let mut final_status = "starting".to_string();
    if !args.no_wait {
        ctx.out
            .status(&format!("Waiting for {} to start...", resolved.instance_uuid));
        final_status = client
            .wait_for_pro_status(
                &resolved.instance_uuid,
                &["running"],
                Duration::from_secs(args.wait_timeout_secs),
                Duration::from_secs(3),
            )
            .await?;
    }

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "status": final_status,
            "has_start_command": start_command.is_some(),
        }))?;
    } else {
        ctx.out.println(&format!(
            "Instance {} ({}) powered on (status: {final_status}).",
            resolved.name, resolved.instance_uuid
        ));
    }
    Ok(())
}

pub async fn handle_power_off(ctx: &CliContext, args: PowerOffArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;

    client.power_off_pro(&resolved.instance_uuid).await?;

    let mut final_status = "shutting_down".to_string();
    if !args.no_wait {
        ctx.out.status(&format!(
            "Waiting for {} to power off...",
            resolved.instance_uuid
        ));
        final_status = client
            .wait_for_pro_status(
                &resolved.instance_uuid,
                &["shutdown", "stopped"],
                Duration::from_secs(args.wait_timeout_secs),
                Duration::from_secs(2),
            )
            .await?;
    }

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "status": final_status,
        }))?;
    } else {
        ctx.out.println(&format!(
            "Instance {} ({}) powered off (status: {final_status}).",
            resolved.name, resolved.instance_uuid
        ));
    }
    Ok(())
}

pub async fn handle_stop(ctx: &CliContext, args: StopArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;

    // Require --yes in non-interactive mode unless --json is passed or stdin is TTY and user confirms.
    if !args.yes && !ctx.out.json && io::stdin().is_terminal() {
        eprint!(
            "Release instance '{}' ({})? This permanently deletes the container [y/N]: ",
            resolved.name, resolved.instance_uuid
        );
        let _ = io::stderr().flush();
        let mut line = String::new();
        io::stdin().read_line(&mut line)?;
        let ans = line.trim().to_ascii_lowercase();
        if ans != "y" && ans != "yes" {
            ctx.out.println("Cancelled.");
            return Ok(());
        }
    }

    ctx.out.status(&format!(
        "Stopping and releasing {} ({})...",
        resolved.name, resolved.instance_uuid
    ));
    client
        .stop_and_release_pro(
            &resolved.instance_uuid,
            Duration::from_secs(args.wait_timeout_secs),
        )
        .await?;
    let _ = store.remove_by_uuid(&resolved.instance_uuid);

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "released": true,
        }))?;
    } else {
        ctx.out.println(&format!(
            "Released instance '{}' ({}).",
            resolved.name, resolved.instance_uuid
        ));
    }
    Ok(())
}

pub async fn handle_url(ctx: &CliContext, args: UrlArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;

    let jupyter_url = if snap.jupyter_domain.is_empty() {
        None
    } else if snap.jupyter_token.is_empty() {
        Some(format!("https://{}", snap.jupyter_domain))
    } else {
        Some(format!(
            "https://{}/?token={}",
            snap.jupyter_domain, snap.jupyter_token
        ))
    };

    let url_6006 = if snap.service_6006_domain.is_empty() {
        None
    } else {
        let proto = if snap.service_6006_port_protocol.is_empty() {
            "https"
        } else {
            snap.service_6006_port_protocol.as_str()
        };
        Some(format!("{proto}://{}", snap.service_6006_domain))
    };

    let url_6008 = if snap.service_6008_domain.is_empty() {
        None
    } else {
        let proto = if snap.service_6008_port_protocol.is_empty() {
            "https"
        } else {
            snap.service_6008_port_protocol.as_str()
        };
        Some(format!("{proto}://{}", snap.service_6008_domain))
    };

    if args.open
        && let Some(url) = &jupyter_url
    {
        open::that(url).map_err(|e| AppError::Other(format!("Failed to open browser: {e}")))?;
    }

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "jupyter_url": jupyter_url,
            "service_6006_url": url_6006,
            "service_6008_url": url_6008,
        }))?;
    } else {
        if let Some(u) = &jupyter_url {
            ctx.out.println(&format!("JupyterLab: {u}"));
        }
        if let Some(u) = &url_6006 {
            ctx.out.println(&format!("Port 6006:  {u}"));
        }
        if let Some(u) = &url_6008 {
            ctx.out.println(&format!("Port 6008:  {u}"));
        }
        if jupyter_url.is_none() && url_6006.is_none() && url_6008.is_none() {
            ctx.out
                .println("No service URLs available yet (instance may still be starting).");
        }
    }
    Ok(())
}

pub async fn handle_boot_log(ctx: &CliContext, args: BootLogArgs) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let store = ctx.session_store();
    let resolved = resolve_session(args.session.as_deref(), &profile, &store, &client).await?;
    let snap = client.get_pro_snapshot(&resolved.instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(30)).await?;

    let read_cmd = match args.tail {
        Some(n) => format!(
            "if [ -f {START_EXIT_PATH} ]; then echo \"__EXIT__=$(cat {START_EXIT_PATH})\"; fi; if [ -f {START_LOG_PATH} ]; then tail -n {n} {START_LOG_PATH}; fi"
        ),
        None => format!(
            "if [ -f {START_EXIT_PATH} ]; then echo \"__EXIT__=$(cat {START_EXIT_PATH})\"; fi; if [ -f {START_LOG_PATH} ]; then cat {START_LOG_PATH}; fi"
        ),
    };

    let out = ssh.exec_streaming(&read_cmd, None, None, true).await?;
    ssh.close().await;

    let mut exit_code: Option<i32> = None;
    let mut lines = Vec::new();
    for line in out.stdout.lines() {
        if let Some(rest) = line.strip_prefix("__EXIT__=") {
            exit_code = rest.trim().parse::<i32>().ok();
        } else {
            lines.push(line);
        }
    }
    let log_text = lines.join("\n");

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "session": resolved.name,
            "instance_uuid": resolved.instance_uuid,
            "boot_exit_code": exit_code,
            "log_path": START_LOG_PATH,
            "log": log_text,
        }))?;
    } else {
        if let Some(code) = exit_code {
            ctx.out.status(&format!("Boot command exit code: {code}"));
        }
        if log_text.is_empty() {
            ctx.out.println("(no boot log recorded yet)");
        } else {
            println!("{log_text}");
        }
    }
    Ok(())
}
