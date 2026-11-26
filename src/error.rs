use serde_json::{Value, json};
use thiserror::Error;

/// Unified application error type with deterministic exit codes and JSON serialization
/// for agent automation.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Ambiguous session: {0}")]
    AmbiguousSession(String),

    #[error("Remote command exited with code {code}: {message}")]
    RemoteExit { code: i32, message: String },

    #[error("Remote command exited with code {0}")]
    ReportedExit(i32),

    #[error("Timed out: {0}")]
    Timeout(String),

    #[error("Invalid input: {0}")]
    Validation(String),

    #[error("AutoDL API error ({code}): {msg}")]
    Api {
        code: String,
        msg: String,
        request_id: Option<String>,
        http_status: Option<u16>,
    },

    #[error("Credential store error: {0}")]
    Keyring(String),

    #[error("SSH error: {0}")]
    Ssh(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// Deterministic exit code for CLI callers and AI agents:
    /// - 0: success (not an AppError)
    /// - 1: general/API/SSH/IO error
    /// - 2: validation/argument error
    /// - 3: authentication/authorization error
    /// - 4: not found / ambiguous session
    /// - 124: timeout
    /// - remote exit code (clamped 1..=255) for RemoteExit
    pub fn exit_code(&self) -> i32 {
        match self {
            AppError::Auth(_) => 3,
            AppError::NotFound(_) | AppError::AmbiguousSession(_) => 4,
            AppError::RemoteExit { code, .. } | AppError::ReportedExit(code) => {
                if *code <= 0 {
                    1
                } else if *code > 255 {
                    255
                } else {
                    *code
                }
            }
            AppError::Timeout(_) => 124,
            AppError::Validation(_) => 2,
            AppError::Api {
                code, http_status, ..
            } => {
                if Self::is_auth_api_error(code, *http_status) {
                    3
                } else {
                    1
                }
            }
            AppError::Keyring(_)
            | AppError::Ssh(_)
            | AppError::Io(_)
            | AppError::Http(_)
            | AppError::Other(_) => 1,
        }
    }

    /// Machine-readable kind string for JSON error payloads.
    pub fn kind(&self) -> &'static str {
        match self {
            AppError::Auth(_) => "auth_error",
            AppError::NotFound(_) => "not_found",
            AppError::AmbiguousSession(_) => "ambiguous_session",
            AppError::RemoteExit { .. } | AppError::ReportedExit(_) => "remote_exit",
            AppError::Timeout(_) => "timeout",
            AppError::Validation(_) => "validation_error",
            AppError::Api {
                code, http_status, ..
            } => {
                if Self::is_auth_api_error(code, *http_status) {
                    "auth_error"
                } else {
                    "api_error"
                }
            }
            AppError::Keyring(_) => "keyring_error",
            AppError::Ssh(_) => "ssh_error",
            AppError::Io(_) => "io_error",
            AppError::Http(_) => "http_error",
            AppError::Other(_) => "error",
        }
    }

    fn is_auth_api_error(code: &str, http_status: Option<u16>) -> bool {
        if matches!(http_status, Some(401 | 403)) {
            return true;
        }
        let lower = code.to_ascii_lowercase();
        lower.contains("auth") || lower.contains("token") || lower.contains("permission")
    }

    /// Structured JSON error envelope printed when `--json` is enabled.
    pub fn to_json(&self) -> Value {
        match self {
            AppError::Api {
                code,
                msg,
                request_id,
                http_status,
            } => json!({
                "ok": false,
                "error": {
                    "kind": self.kind(),
                    "message": self.to_string(),
                    "code": code,
                    "api_msg": msg,
                    "request_id": request_id,
                    "http_status": http_status,
                    "exit_code": self.exit_code(),
                }
            }),
            AppError::RemoteExit { code, message } => json!({
                "ok": false,
                "error": {
                    "kind": self.kind(),
                    "message": message,
                    "remote_exit_code": code,
                    "exit_code": self.exit_code(),
                }
            }),
            _ => json!({
                "ok": false,
                "error": {
                    "kind": self.kind(),
                    "message": self.to_string(),
                    "exit_code": self.exit_code(),
                }
            }),
        }
    }
}
