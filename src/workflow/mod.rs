pub mod run;

use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;

use crate::error::AppError;
use crate::ssh::transfer::shell_escape;

pub const START_LOG_PATH: &str = "/var/log/autodl-start.log";
pub const START_EXIT_PATH: &str = "/tmp/autodl_start.exit";

/// Build an AutoDL `start_command` for instance creation or `power_on`.
///
/// - If `start_script` is given, its contents are base64-encoded into a self-contained
///   `sh -c` command that decodes to `/tmp/autodl_start_script`, runs with `python3` (`.py`)
///   or `bash`, and logs stdout/stderr to `/var/log/autodl-start.log`.
/// - If `start_cmd` is given, it is wrapped with logging to `/var/log/autodl-start.log`
///   unless `raw` is `true`.
/// - If `auto_shutdown` is `true`, appends `; sync; shutdown -h now || poweroff` after recording
///   the exit code into `/tmp/autodl_start.exit`.
pub fn build_start_command(
    start_cmd: Option<&str>,
    start_script: Option<&Path>,
    auto_shutdown: bool,
    raw: bool,
) -> Result<Option<String>, AppError> {
    if start_cmd.is_some() && start_script.is_some() {
        return Err(AppError::Validation(
            "Pass either --start-cmd or --start-script, not both".into(),
        ));
    }

    if let Some(script_path) = start_script {
        let bytes = fs::read(script_path).map_err(|e| {
            AppError::Validation(format!(
                "Failed to read start script '{}': {e}",
                script_path.display()
            ))
        })?;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let is_python = script_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("py"))
            .unwrap_or(false);

        let runner = if is_python {
            "python3 /tmp/autodl_start_script"
        } else {
            "chmod +x /tmp/autodl_start_script && bash /tmp/autodl_start_script"
        };

        let shutdown_tail = if auto_shutdown {
            " sync; shutdown -h now || poweroff;"
        } else {
            ""
        };

        let inner = format!(
            "echo {b64} | base64 -d > /tmp/autodl_start_script && ({runner}) > {START_LOG_PATH} 2>&1; rc=$?; echo \"$rc\" > {START_EXIT_PATH};{shutdown_tail} exit \"$rc\"",
        );
        return Ok(Some(format!("sh -c {}", shell_escape(&inner))));
    }

    let Some(cmd) = start_cmd.map(str::trim).filter(|s| !s.is_empty()) else {
        if auto_shutdown {
            return Err(AppError::Validation(
                "--auto-shutdown-on-exit requires --start-cmd or --start-script".into(),
            ));
        }
        return Ok(None);
    };

    if raw && !auto_shutdown {
        return Ok(Some(cmd.to_string()));
    }

    let shutdown_tail = if auto_shutdown {
        " sync; shutdown -h now || poweroff;"
    } else {
        ""
    };
    let inner = format!(
        "({cmd}) > {START_LOG_PATH} 2>&1; rc=$?; echo \"$rc\" > {START_EXIT_PATH};{shutdown_tail} exit \"$rc\"",
    );
    Ok(Some(format!("sh -c {}", shell_escape(&inner))))
}

/// Parse `--download REMOTE:LOCAL` (Windows-safe: handles `REMOTE:C:\path\out.bin`).
pub fn parse_download_spec(spec: &str) -> Result<(String, PathBuf), AppError> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "Download spec cannot be empty (expected REMOTE:LOCAL)".into(),
        ));
    }
    if let Some((remote, local)) = trimmed.split_once(':') {
        let r = remote.trim();
        let l = local.trim();
        if r.is_empty() || l.is_empty() {
            return Err(AppError::Validation(format!(
                "Invalid download spec '{spec}': expected REMOTE:LOCAL"
            )));
        }
        return Ok((r.to_string(), PathBuf::from(l)));
    }
    // Single path: download into current directory with same basename
    let fname = trimmed
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("output");
    Ok((trimmed.to_string(), PathBuf::from(fname)))
}

/// Parse `KEY=VALUE` environment variable pairs.
pub fn parse_env_pair(spec: &str) -> Result<(String, String), AppError> {
    let Some((k, v)) = spec.split_once('=') else {
        return Err(AppError::Validation(format!(
            "Invalid --env '{spec}': expected KEY=VALUE"
        )));
    };
    let key = k.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(AppError::Validation(format!(
            "Invalid environment variable name '{key}'"
        )));
    }
    Ok((key.to_string(), v.to_string()))
}

/// Wrap a remote command with `cd <workdir>` and `export KEY=VALUE`.
pub fn build_remote_exec_command(
    workdir: Option<&str>,
    envs: &[(String, String)],
    cmd: &str,
) -> String {
    let mut prefix = String::new();
    if let Some(wd) = workdir.map(str::trim).filter(|s| !s.is_empty()) {
        prefix.push_str(&format!("mkdir -p {0} && cd {0} && ", shell_escape(wd)));
    }
    for (k, v) in envs {
        prefix.push_str(&format!("export {k}={}; ", shell_escape(v)));
    }
    format!("{prefix}{cmd}")
}

/// Build a fast `uv pip install` command with automatic `pip install` fallback.
pub fn build_install_command(
    packages: &[String],
    requirements_remote: Option<&str>,
    upgrade: bool,
) -> Result<String, AppError> {
    if packages.is_empty() && requirements_remote.is_none() {
        return Err(AppError::Validation(
            "Specify packages or -r/--requirements to install".into(),
        ));
    }

    let mut args = Vec::new();
    if upgrade {
        args.push("--upgrade".to_string());
    }
    if let Some(req) = requirements_remote {
        args.push("-r".to_string());
        args.push(shell_escape(req));
    }
    for pkg in packages {
        args.push(shell_escape(pkg));
    }
    let joined = args.join(" ");
    Ok(format!(
        "if command -v uv >/dev/null 2>&1; then uv pip install --system {joined}; else pip install {joined}; fi"
    ))
}

/// Build remote execution command for an uploaded script file and forwarded args.
pub fn build_script_run_command(remote_script: &str, script_args: &[String]) -> String {
    let escaped_script = shell_escape(remote_script);
    let escaped_args: Vec<String> = script_args.iter().map(|a| shell_escape(a)).collect();
    let args_tail = if escaped_args.is_empty() {
        String::new()
    } else {
        format!(" {}", escaped_args.join(" "))
    };

    if remote_script.ends_with(".py") {
        format!("python3 -u {escaped_script}{args_tail}")
    } else if remote_script.ends_with(".sh") {
        format!("bash {escaped_script}{args_tail}")
    } else {
        format!("chmod +x {escaped_script} && {escaped_script}{args_tail}")
    }
}
