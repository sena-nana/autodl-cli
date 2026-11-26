use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;

use crate::api::AutoDlClient;
use crate::api::types::CreateProInstanceReq;
use crate::catalog::{
    DEFAULT_CUDA_VERSION, DEFAULT_GPU_SPEC, DEFAULT_IMAGE_UUID, resolve_gpu_spec,
    resolve_image_uuid,
};
use crate::error::AppError;
use crate::output::Output;
use crate::ssh::transfer::{
    PackOptions, download_path, sync_directory, upload_file_sftp,
};
use crate::ssh::{SshEndpoint, SshSession};
use crate::state::{DEFAULT_REMOTE_DIR, SessionRecord, SessionStore, generate_session_name};

use super::{
    build_install_command, build_remote_exec_command, build_script_run_command, build_start_command,
};

#[derive(Debug, Clone)]
pub struct EphemeralRunOptions {
    pub profile: String,
    pub session_name: Option<String>,
    pub gpu: Option<String>,
    pub gpu_count: u32,
    pub image: Option<String>,
    pub cuda_version: Option<i32>,
    pub regions: Vec<String>,
    pub expand_disk_gb: u32,
    pub script: Option<PathBuf>,
    pub script_args: Vec<String>,
    pub cmd: Option<String>,
    pub sync_dir: Option<PathBuf>,
    pub sync_dest: String,
    pub pack_options: PackOptions,
    pub install_packages: Vec<String>,
    pub requirements_file: Option<PathBuf>,
    pub downloads: Vec<(String, PathBuf)>,
    pub download_on_error: bool,
    pub envs: Vec<(String, String)>,
    pub workdir: Option<String>,
    pub wait_timeout_secs: u64,
    pub idle_timeout_secs: u64,
    pub keep: bool,
    pub power_off_only: bool,
    pub save_image: Option<String>,
    pub start_cmd: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadArtifact {
    pub remote: String,
    pub local: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub ok: bool,
    pub instance_uuid: String,
    pub session_name: String,
    pub gpu_spec: String,
    pub gpu_count: u32,
    pub image_uuid: String,
    pub exit_code: i32,
    pub released: bool,
    pub powered_off: bool,
    pub kept: bool,
    pub duration_secs: f64,
    pub synced_files: Option<usize>,
    pub downloads: Vec<DownloadArtifact>,
    pub saved_image_uuid: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

/// Execute the full ephemeral AutoDL lifecycle:
/// 1. Provision Pro instance
/// 2. Wait for `running` & connect SSH
/// 3. Optional code directory sync + script upload + dependency install
/// 4. Stream execution with idle timeout
/// 5. Retrieve artifact downloads & optional save-image
/// 6. Guaranteed teardown (release by default, or power-off, unless `--keep`)
pub async fn execute_ephemeral_run(
    api: &AutoDlClient,
    store: &SessionStore,
    opts: EphemeralRunOptions,
    out: &Output,
) -> Result<RunReport, AppError> {
    if opts.script.is_none() && opts.cmd.is_none() {
        return Err(AppError::Validation(
            "Provide either a local SCRIPT or --cmd <COMMAND>".into(),
        ));
    }
    if let Some(script_path) = &opts.script
        && !script_path.is_file()
    {
        return Err(AppError::NotFound(format!(
            "Local script '{}' not found",
            script_path.display()
        )));
    }
    if let Some(req_file) = &opts.requirements_file
        && !req_file.is_file()
    {
        return Err(AppError::NotFound(format!(
            "Requirements file '{}' not found",
            req_file.display()
        )));
    }

    let start_instant = Instant::now();
    let session_name = opts
        .session_name
        .clone()
        .unwrap_or_else(|| generate_session_name("run"));

    let (gpu_spec, known_gpu) = match opts.gpu.as_deref() {
        Some(g) => resolve_gpu_spec(g),
        None => (DEFAULT_GPU_SPEC.to_string(), true),
    };
    if !known_gpu {
        out.warn(&format!(
            "GPU spec '{gpu_spec}' is not in the built-in catalog; passing through raw to AutoDL API"
        ));
    }

    let (image_uuid, suggested_cuda) = match opts.image.as_deref() {
        Some(img) => resolve_image_uuid(img),
        None => (DEFAULT_IMAGE_UUID.to_string(), Some(DEFAULT_CUDA_VERSION)),
    };
    let cuda_v_from = opts
        .cuda_version
        .or(suggested_cuda)
        .unwrap_or(DEFAULT_CUDA_VERSION);

    let boot_start_cmd = build_start_command(opts.start_cmd.as_deref(), None, false, true)?;

    let create_req = CreateProInstanceReq {
        data_center_list: if opts.regions.is_empty() {
            None
        } else {
            Some(opts.regions.clone())
        },
        req_gpu_amount: opts.gpu_count,
        expand_system_disk_by_gb: opts.expand_disk_gb,
        gpu_spec_uuid: gpu_spec.clone(),
        image_uuid: image_uuid.clone(),
        cuda_v_from,
        instance_name: Some(session_name.clone()),
        start_command: boot_start_cmd,
    };

    out.info(&format!(
        "Provisioning AutoDL instance '{session_name}' ({gpu_spec} x{}, image {image_uuid})...",
        opts.gpu_count
    ));
    let instance_uuid = api.create_pro_instance(&create_req).await?;
    out.info(&format!("Created instance {instance_uuid}"));

    // Register immediately so even an abrupt interruption leaves a trackable session.
    store.upsert(SessionRecord {
        name: session_name.clone(),
        instance_uuid: instance_uuid.clone(),
        profile: opts.profile.clone(),
        gpu_spec: gpu_spec.clone(),
        gpu_count: opts.gpu_count,
        image_uuid: image_uuid.clone(),
        region: opts.regions.first().cloned(),
        created_at: Utc::now(),
        default_remote_dir: DEFAULT_REMOTE_DIR.to_string(),
    })?;

    let run_result = tokio::select! {
        res = run_inside_instance(api, &instance_uuid, &opts, out) => res,
        _ = tokio::signal::ctrl_c() => {
            out.warn("Ctrl+C received; initiating cleanup...");
            Err(AppError::Other("Interrupted by Ctrl+C".into()))
        }
    };

    // Cleanup phase (always runs unless --keep)
    let mut released = false;
    let mut powered_off = false;
    let kept = opts.keep;

    if !opts.keep {
        if opts.power_off_only {
            out.info(&format!("Powering off instance {instance_uuid} (--power-off)..."));
            match api.power_off_pro(&instance_uuid).await {
                Ok(()) => {
                    let _ = api
                        .wait_for_pro_status(
                            &instance_uuid,
                            &["shutdown", "stopped"],
                            Duration::from_secs(120),
                            Duration::from_secs(2),
                        )
                        .await;
                    powered_off = true;
                }
                Err(e) => {
                    out.warn(&format!(
                        "Failed to power off {instance_uuid}: {e}. Run `autodl power-off -s {instance_uuid}` manually."
                    ));
                }
            }
        } else {
            out.info(&format!("Releasing ephemeral instance {instance_uuid}..."));
            match api
                .stop_and_release_pro(&instance_uuid, Duration::from_secs(120))
                .await
            {
                Ok(()) => {
                    released = true;
                    let _ = store.remove_by_uuid(&instance_uuid);
                    out.info(&format!("Released {instance_uuid}"));
                }
                Err(e) => {
                    out.warn(&format!(
                        "Auto-release failed for {instance_uuid}: {e}. IMPORTANT: run `autodl stop -s {instance_uuid}` to avoid ongoing charges!"
                    ));
                }
            }
        }
    } else {
        out.info(&format!(
            "Keeping instance {instance_uuid} active as session '{session_name}' (--keep)"
        ));
    }

    let duration_secs = start_instant.elapsed().as_secs_f64();

    match run_result {
        Ok(inner) => {
            let ok = inner.exit_code == 0;
            let report = RunReport {
                ok,
                instance_uuid,
                session_name,
                gpu_spec,
                gpu_count: opts.gpu_count,
                image_uuid,
                exit_code: inner.exit_code,
                released,
                powered_off,
                kept,
                duration_secs,
                synced_files: inner.synced_files,
                downloads: inner.downloads,
                saved_image_uuid: inner.saved_image_uuid,
                stdout: inner.stdout,
                stderr: inner.stderr,
            };

            if out.json {
                out.emit_json(&report)?;
                if !ok {
                    return Err(AppError::ReportedExit(inner.exit_code));
                }
            } else if !ok {
                return Err(AppError::RemoteExit {
                    code: inner.exit_code,
                    message: format!("Remote job exited with status {}", inner.exit_code),
                });
            }
            Ok(report)
        }
        Err(e) => Err(e),
    }
}

struct InnerRunOutput {
    exit_code: i32,
    stdout: String,
    stderr: String,
    synced_files: Option<usize>,
    downloads: Vec<DownloadArtifact>,
    saved_image_uuid: Option<String>,
}

async fn run_inside_instance(
    api: &AutoDlClient,
    instance_uuid: &str,
    opts: &EphemeralRunOptions,
    out: &Output,
) -> Result<InnerRunOutput, AppError> {
    out.info(&format!("Waiting for {instance_uuid} to enter 'running' state..."));
    api.wait_for_pro_status(
        instance_uuid,
        &["running"],
        Duration::from_secs(opts.wait_timeout_secs),
        Duration::from_secs(3),
    )
    .await?;

    let snap = api.get_pro_snapshot(instance_uuid).await?;
    let endpoint = SshEndpoint::from_snapshot(&snap)?;
    out.info(&format!(
        "Connecting SSH to {}@{}:{}...",
        endpoint.user, endpoint.host, endpoint.port
    ));
    let ssh = SshSession::connect_with_retry(endpoint, Duration::from_secs(90)).await?;

    let remote_workdir = opts
        .workdir
        .clone()
        .unwrap_or_else(|| opts.sync_dest.clone());

    let mut synced_files = None;
    if let Some(sync_dir) = &opts.sync_dir {
        out.info(&format!(
            "Syncing '{}' -> {}:{}...",
            sync_dir.display(),
            instance_uuid,
            opts.sync_dest
        ));
        let stats =
            sync_directory(&ssh, sync_dir, &opts.sync_dest, &opts.pack_options, false).await?;
        out.info(&format!(
            "Synced {} files ({} bytes compressed)",
            stats.files, stats.bytes_compressed
        ));
        synced_files = Some(stats.files);
    }

    // Upload requirements.txt and/or script via SFTP
    let mut remote_req_path: Option<String> = None;
    let mut remote_script_path: Option<String> = None;

    if opts.requirements_file.is_some() || opts.script.is_some() {
        let sftp = ssh.open_sftp().await?;
        if let Some(req_local) = &opts.requirements_file {
            let fname = req_local
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("requirements.txt");
            let rpath = format!("{}/{}", remote_workdir.trim_end_matches('/'), fname);
            upload_file_sftp(&sftp, req_local, &rpath).await?;
            remote_req_path = Some(rpath);
        }
        if let Some(script_local) = &opts.script {
            let fname = script_local
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("run_script.py");
            let rpath = format!("{}/{}", remote_workdir.trim_end_matches('/'), fname);
            upload_file_sftp(&sftp, script_local, &rpath).await?;
            remote_script_path = Some(rpath);
        }
        let _ = sftp.close().await;
    }

    if !opts.install_packages.is_empty() || remote_req_path.is_some() {
        out.info("Installing dependencies on remote instance...");
        let install_cmd = build_install_command(
            &opts.install_packages,
            remote_req_path.as_deref(),
            false,
        )?;
        let wrapped = build_remote_exec_command(Some(&remote_workdir), &opts.envs, &install_cmd);
        let install_res = ssh
            .exec_streaming(&wrapped, None, Some(Duration::from_secs(600)), out.json)
            .await?;
        if install_res.exit_code != 0 {
            return Err(AppError::RemoteExit {
                code: install_res.exit_code,
                message: format!(
                    "Package installation failed (exit {}): {}",
                    install_res.exit_code,
                    install_res.stderr.trim()
                ),
            });
        }
    }

    let exec_cmd = if let Some(raw_cmd) = &opts.cmd {
        raw_cmd.clone()
    } else if let Some(rscript) = &remote_script_path {
        build_script_run_command(rscript, &opts.script_args)
    } else {
        unreachable!("validated above")
    };

    let full_cmd = build_remote_exec_command(Some(&remote_workdir), &opts.envs, &exec_cmd);
    let idle_limit = if opts.idle_timeout_secs == 0 {
        None
    } else {
        Some(Duration::from_secs(opts.idle_timeout_secs))
    };

    out.info(&format!("Executing on {instance_uuid}: {exec_cmd}"));
    let exec_out = ssh
        .exec_streaming(&full_cmd, None, idle_limit, out.json)
        .await?;

    let mut downloads = Vec::new();
    if exec_out.exit_code == 0 || opts.download_on_error {
        for (remote_src, local_dst) in &opts.downloads {
            let resolved_remote = if remote_src.starts_with('/') {
                remote_src.clone()
            } else {
                format!(
                    "{}/{}",
                    remote_workdir.trim_end_matches('/'),
                    remote_src.trim_start_matches("./")
                )
            };
            out.info(&format!(
                "Downloading {} -> {}...",
                resolved_remote,
                local_dst.display()
            ));
            let (saved_path, bytes) =
                download_path(&ssh, &resolved_remote, local_dst, true).await?;
            downloads.push(DownloadArtifact {
                remote: resolved_remote,
                local: saved_path.display().to_string(),
                bytes,
            });
        }
    }

    ssh.close().await;

    let mut saved_image_uuid = None;
    if let Some(img_name) = &opts.save_image {
        out.info(&format!("Saving instance image '{img_name}'..."));
        let resp = api.save_pro_image(instance_uuid, img_name).await?;
        saved_image_uuid = Some(resp.image_uuid);
    }

    Ok(InnerRunOutput {
        exit_code: exec_out.exit_code,
        stdout: exec_out.stdout,
        stderr: exec_out.stderr,
        synced_files,
        downloads,
        saved_image_uuid,
    })
}
