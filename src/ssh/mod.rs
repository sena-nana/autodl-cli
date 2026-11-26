pub mod storage;
pub mod transfer;

use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use russh::client::{self, Handle};
use russh::keys::ssh_key::PublicKey;
use russh::{ChannelMsg, Disconnect};
use russh_sftp::client::SftpSession;
use serde::Serialize;
use tokio::io::AsyncWriteExt;

use crate::api::types::ProInstanceSnapshot;
use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct SshEndpoint {
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(skip_serializing)]
    pub password: String,
}

impl SshEndpoint {
    /// Extract SSH endpoint details from a Pro instance snapshot.
    /// Uses `proxy_host`, `ssh_port`, `root_password` with fallback parsing on `ssh_command`.
    pub fn from_snapshot(snap: &ProInstanceSnapshot) -> Result<Self, AppError> {
        let mut host = snap.proxy_host.trim().to_string();
        let mut port = snap.ssh_port;
        let mut user = "root".to_string();

        if (host.is_empty() || port == 0) && !snap.ssh_command.trim().is_empty() {
            let parsed = parse_ssh_command(&snap.ssh_command);
            if host.is_empty()
                && let Some(h) = parsed.0
            {
                host = h;
            }
            if port == 0
                && let Some(p) = parsed.1
            {
                port = p;
            }
            if let Some(u) = parsed.2 {
                user = u;
            }
        } else if !snap.ssh_command.trim().is_empty()
            && let (_, _, Some(u)) = parse_ssh_command(&snap.ssh_command)
        {
            user = u;
        }

        if host.is_empty() || port == 0 {
            return Err(AppError::Ssh(
                "Instance snapshot does not contain SSH host/port yet (is the instance running?)"
                    .into(),
            ));
        }
        if snap.root_password.is_empty() {
            return Err(AppError::Ssh(
                "Instance snapshot does not contain root_password".into(),
            ));
        }

        Ok(Self {
            host,
            port,
            user,
            password: snap.root_password.clone(),
        })
    }

    pub fn ssh_command_display(&self) -> String {
        format!("ssh -p {} {}@{}", self.port, self.user, self.host)
    }
}

/// Parse `ssh -p 34222 root@connect.xxx.autodl.com` into `(host, port, user)`.
pub fn parse_ssh_command(cmd: &str) -> (Option<String>, Option<u16>, Option<String>) {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let mut host = None;
    let mut port = None;
    let mut user = None;

    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        if tok == "-p" && i + 1 < tokens.len() {
            port = tokens[i + 1].parse::<u16>().ok();
            i += 2;
            continue;
        }
        if let Some(p_str) = tok.strip_prefix("-p")
            && !p_str.is_empty()
            && let Ok(p) = p_str.parse::<u16>()
        {
            port = Some(p);
            i += 1;
            continue;
        }
        if let Some((u, h)) = tok.split_once('@')
            && !u.is_empty()
            && !h.is_empty()
        {
            user = Some(u.to_string());
            host = Some(h.to_string());
        }
        i += 1;
    }

    (host, port, user)
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // AutoDL container host keys are ephemeral per container/region proxy.
        Ok(true)
    }
}

pub struct SshSession {
    handle: Handle<ClientHandler>,
    endpoint: SshEndpoint,
}

impl SshSession {
    pub fn endpoint(&self) -> &SshEndpoint {
        &self.endpoint
    }

    pub async fn connect(endpoint: SshEndpoint, timeout: Duration) -> Result<Self, AppError> {
        let config = Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(3600)),
            keepalive_interval: Some(Duration::from_secs(20)),
            ..Default::default()
        });

        let addr = (endpoint.host.as_str(), endpoint.port);
        let mut handle = tokio::time::timeout(timeout, client::connect(config, addr, ClientHandler))
            .await
            .map_err(|_| {
                AppError::Timeout(format!(
                    "SSH TCP/handshake timed out connecting to {}:{}",
                    endpoint.host, endpoint.port
                ))
            })?
            .map_err(|e| {
                AppError::Ssh(format!(
                    "SSH connection failed to {}:{}: {e}",
                    endpoint.host, endpoint.port
                ))
            })?;

        let auth_res = tokio::time::timeout(
            timeout,
            handle.authenticate_password(&endpoint.user, &endpoint.password),
        )
        .await
        .map_err(|_| AppError::Timeout("SSH password authentication timed out".into()))?
        .map_err(|e| AppError::Ssh(format!("SSH authentication error: {e}")))?;

        if !auth_res.success() {
            return Err(AppError::Auth(format!(
                "SSH password authentication rejected for {}@{}:{}",
                endpoint.user, endpoint.host, endpoint.port
            )));
        }

        Ok(Self { handle, endpoint })
    }

    /// Connect with retries while a freshly booted container's sshd starts up.
    pub async fn connect_with_retry(
        endpoint: SshEndpoint,
        total_timeout: Duration,
    ) -> Result<Self, AppError> {
        let deadline = Instant::now() + total_timeout;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AppError::Timeout(format!(
                    "Timed out waiting for SSH on {}:{} after {}s",
                    endpoint.host,
                    endpoint.port,
                    total_timeout.as_secs()
                )));
            }
            let per_try = remaining.min(Duration::from_secs(12));
            match Self::connect(endpoint.clone(), per_try).await {
                Ok(sess) => return Ok(sess),
                Err(AppError::Auth(msg)) if attempt >= 3 => return Err(AppError::Auth(msg)),
                Err(e) => {
                    if Instant::now() >= deadline {
                        return Err(e);
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    }

    /// Execute a command on the remote container, streaming output unless `capture_only` is true.
    /// `idle_timeout` resets on every received stdout/stderr chunk.
    pub async fn exec_streaming(
        &self,
        command: &str,
        stdin_data: Option<Vec<u8>>,
        idle_timeout: Option<Duration>,
        capture_only: bool,
    ) -> Result<ExecOutput, AppError> {
        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| AppError::Ssh(format!("Failed to open SSH session channel: {e}")))?;

        channel
            .exec(true, command.as_bytes())
            .await
            .map_err(|e| AppError::Ssh(format!("Failed to execute remote command: {e}")))?;

        let writer_task = if let Some(data) = stdin_data {
            let mut writer = channel.make_writer();
            Some(tokio::spawn(async move {
                let write_res = writer.write_all(&data).await;
                let _ = writer.shutdown().await;
                write_res
            }))
        } else {
            None
        };

        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        let mut exit_code: Option<i32> = None;

        loop {
            let next_msg = if let Some(limit) = idle_timeout {
                match tokio::time::timeout(limit, channel.wait()).await {
                    Ok(msg) => msg,
                    Err(_) => {
                        let _ = channel.close().await;
                        return Err(AppError::Timeout(format!(
                            "Remote command produced no output for {}s",
                            limit.as_secs()
                        )));
                    }
                }
            } else {
                channel.wait().await
            };

            let Some(msg) = next_msg else {
                break;
            };

            match msg {
                ChannelMsg::Data { data } => {
                    stdout_buf.extend_from_slice(&data);
                    if !capture_only {
                        let mut out = io::stdout().lock();
                        let _ = out.write_all(&data);
                        let _ = out.flush();
                    }
                }
                ChannelMsg::ExtendedData { ext, data } => {
                    if ext == 1 {
                        stderr_buf.extend_from_slice(&data);
                        if !capture_only {
                            let mut err = io::stderr().lock();
                            let _ = err.write_all(&data);
                            let _ = err.flush();
                        }
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = Some(exit_status as i32);
                }
                ChannelMsg::ExitSignal { signal_name, .. } => {
                    let msg_str = format!("Remote process killed by signal {signal_name:?}");
                    stderr_buf.extend_from_slice(msg_str.as_bytes());
                    if exit_code.is_none() {
                        exit_code = Some(143);
                    }
                }
                ChannelMsg::Close | ChannelMsg::Eof => {
                    if matches!(msg, ChannelMsg::Close) {
                        break;
                    }
                }
                _ => {}
            }
        }

        if let Some(handle) = writer_task {
            let join_res = handle
                .await
                .map_err(|e| AppError::Ssh(format!("SSH stdin task panicked: {e}")))?;
            if let Err(io_err) = join_res {
                return Err(AppError::Ssh(format!(
                    "Failed writing data to remote stdin: {io_err}"
                )));
            }
        }

        Ok(ExecOutput {
            exit_code: exit_code.unwrap_or(0),
            stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
        })
    }

    /// Open an SFTP subsystem session over the existing SSH connection.
    pub async fn open_sftp(&self) -> Result<SftpSession, AppError> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| AppError::Ssh(format!("Failed to open SSH channel for SFTP: {e}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|e| AppError::Ssh(format!("Failed to start SFTP subsystem: {e}")))?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|e| AppError::Ssh(format!("SFTP handshake failed: {e}")))?;
        Ok(sftp)
    }

    /// Open an interactive PTY shell over SSH.
    pub async fn open_interactive_shell(&self) -> Result<i32, AppError> {
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| AppError::Ssh(format!("Failed to open SSH session channel: {e}")))?;

        let (cols, rows) = crossterm::terminal::size().unwrap_or((100, 32));
        channel
            .request_pty(
                true,
                "xterm-256color",
                cols as u32,
                rows as u32,
                0,
                0,
                &[],
            )
            .await
            .map_err(|e| AppError::Ssh(format!("Failed to request PTY: {e}")))?;

        channel
            .request_shell(true)
            .await
            .map_err(|e| AppError::Ssh(format!("Failed to start remote shell: {e}")))?;

        let _raw_guard = RawModeGuard::enable()?;

        let (mut reader, writer) = channel.split();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(32);

        // Blocking stdin reader thread so raw terminal input flows smoothly on Windows & Unix.
        std::thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut buf = [0u8; 1024];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.blocking_send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        let mut ssh_writer = writer.make_writer();
        let write_handle = tokio::spawn(async move {
            while let Some(chunk) = rx.recv().await {
                if ssh_writer.write_all(&chunk).await.is_err() {
                    break;
                }
                let _ = ssh_writer.flush().await;
            }
            let _ = writer.eof().await;
        });

        let mut exit_code = 0i32;
        while let Some(msg) = reader.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    let mut out = io::stdout().lock();
                    let _ = out.write_all(&data);
                    let _ = out.flush();
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    let mut err = io::stderr().lock();
                    let _ = err.write_all(&data);
                    let _ = err.flush();
                }
                ChannelMsg::ExitStatus { exit_status } => {
                    exit_code = exit_status as i32;
                }
                ChannelMsg::Close => break,
                _ => {}
            }
        }

        write_handle.abort();
        Ok(exit_code)
    }

    pub async fn close(self) {
        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "done", "en")
            .await;
    }
}

struct RawModeGuard {
    enabled: bool,
}

impl RawModeGuard {
    fn enable() -> Result<Self, AppError> {
        let enabled = crossterm::terminal::enable_raw_mode().is_ok();
        Ok(Self { enabled })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.enabled {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}
