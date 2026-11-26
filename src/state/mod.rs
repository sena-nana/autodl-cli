use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use rand::Rng;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::api::AutoDlClient;
use crate::error::AppError;

pub const DEFAULT_REMOTE_DIR: &str = "/root/autodl-tmp";

/// Local cached metadata for a tracked AutoDL session.
/// Note: `root_password` is intentionally NOT stored on disk; SSH/SFTP always fetches it fresh
/// from `GET /api/v1/dev/instance/pro/snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub name: String,
    pub instance_uuid: String,
    pub profile: String,
    pub gpu_spec: String,
    pub gpu_count: u32,
    pub image_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default = "default_remote_dir_string")]
    pub default_remote_dir: String,
}

fn default_remote_dir_string() -> String {
    DEFAULT_REMOTE_DIR.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionStateFile {
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolvedSession {
    pub name: String,
    pub instance_uuid: String,
    pub from_local_cache: bool,
    pub default_remote_dir: String,
}

pub struct SessionStore {
    path: PathBuf,
}

impl SessionStore {
    pub fn new(config_dir: &Path) -> Self {
        Self {
            path: config_dir.join("sessions.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<SessionStateFile, AppError> {
        read_json_or_default(&self.path)
    }

    pub fn save(&self, state: &SessionStateFile) -> Result<(), AppError> {
        write_json_atomic(&self.path, state)
    }

    pub fn upsert(&self, record: SessionRecord) -> Result<(), AppError> {
        let mut state = self.load()?;
        // Remove any stale record with the same instance_uuid under a different name.
        state
            .sessions
            .retain(|_, v| v.instance_uuid != record.instance_uuid || v.name == record.name);
        state.sessions.insert(record.name.clone(), record);
        self.save(&state)
    }

    pub fn remove_by_name(&self, name: &str) -> Result<Option<SessionRecord>, AppError> {
        let mut state = self.load()?;
        let removed = state.sessions.remove(name);
        if removed.is_some() {
            self.save(&state)?;
        }
        Ok(removed)
    }

    pub fn remove_by_uuid(&self, uuid: &str) -> Result<Option<SessionRecord>, AppError> {
        let mut state = self.load()?;
        let key = state
            .sessions
            .iter()
            .find(|(_, v)| v.instance_uuid == uuid)
            .map(|(k, _)| k.clone());
        let removed = key.and_then(|k| state.sessions.remove(&k));
        if removed.is_some() {
            self.save(&state)?;
        }
        Ok(removed)
    }

    pub fn list_for_profile(&self, profile: &str) -> Result<Vec<SessionRecord>, AppError> {
        let state = self.load()?;
        Ok(state
            .sessions
            .into_values()
            .filter(|r| r.profile == profile)
            .collect())
    }

    pub fn clear_profile(&self, profile: &str) -> Result<usize, AppError> {
        let mut state = self.load()?;
        let before = state.sessions.len();
        state.sessions.retain(|_, v| v.profile != profile);
        let removed = before.saturating_sub(state.sessions.len());
        if removed > 0 {
            self.save(&state)?;
        }
        Ok(removed)
    }
}

/// Resolve a target session or instance:
/// 1. If `target` starts with `pro-`, treat it directly as an `instance_uuid`.
/// 2. If `target` is a session name, check local `SessionStore` first, then fall back to remote `pro/list` by `name`.
/// 3. If `target` is `None`:
///    - If local `SessionStore` has exactly 1 session for `profile`, pick it.
///    - If local `SessionStore` has >1, return `AmbiguousSession`.
///    - If local `SessionStore` has 0, query remote `pro/list` for running instances; if exactly 1 running instance exists, auto-select it.
pub async fn resolve_session(
    target: Option<&str>,
    profile: &str,
    store: &SessionStore,
    api: &AutoDlClient,
) -> Result<ResolvedSession, AppError> {
    if let Some(raw) = target.map(str::trim).filter(|s| !s.is_empty()) {
        if raw.starts_with("pro-") {
            let local = store
                .list_for_profile(profile)?
                .into_iter()
                .find(|s| s.instance_uuid == raw);
            return Ok(ResolvedSession {
                name: local
                    .as_ref()
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| raw.to_string()),
                instance_uuid: raw.to_string(),
                from_local_cache: local.is_some(),
                default_remote_dir: local
                    .map(|s| s.default_remote_dir)
                    .unwrap_or_else(default_remote_dir_string),
            });
        }

        if let Some(rec) = store
            .list_for_profile(profile)?
            .into_iter()
            .find(|s| s.name == raw)
        {
            return Ok(ResolvedSession {
                name: rec.name,
                instance_uuid: rec.instance_uuid,
                from_local_cache: true,
                default_remote_dir: rec.default_remote_dir,
            });
        }

        // Fall back to remote instance name lookup
        let remote_instances = api.list_all_pro_instances().await?;
        let matches: Vec<_> = remote_instances
            .into_iter()
            .filter(|i| i.name == raw && i.status != "released")
            .collect();
        match matches.len() {
            0 => {
                return Err(AppError::NotFound(format!(
                    "Session or instance '{raw}' not found locally or remotely"
                )));
            }
            1 => {
                let inst = &matches[0];
                return Ok(ResolvedSession {
                    name: inst.name.clone(),
                    instance_uuid: inst.uuid.clone(),
                    from_local_cache: false,
                    default_remote_dir: default_remote_dir_string(),
                });
            }
            _ => {
                let candidates: Vec<String> = matches
                    .iter()
                    .map(|i| format!("{} ({})", i.name, i.uuid))
                    .collect();
                return Err(AppError::AmbiguousSession(format!(
                    "Multiple remote instances named '{raw}': {}. Pass the instance UUID (pro-...) via -s.",
                    candidates.join(", ")
                )));
            }
        }
    }

    let local = store.list_for_profile(profile)?;
    if local.len() == 1 {
        let rec = &local[0];
        return Ok(ResolvedSession {
            name: rec.name.clone(),
            instance_uuid: rec.instance_uuid.clone(),
            from_local_cache: true,
            default_remote_dir: rec.default_remote_dir.clone(),
        });
    }
    if local.len() > 1 {
        let names: Vec<String> = local
            .iter()
            .map(|s| format!("{} ({})", s.name, s.instance_uuid))
            .collect();
        return Err(AppError::AmbiguousSession(format!(
            "Multiple active local sessions: {}. Specify one with -s/--session.",
            names.join(", ")
        )));
    }

    // No local sessions: check remote running instances for single-session auto-selection
    let remote = api.list_all_pro_instances().await?;
    let running: Vec<_> = remote
        .into_iter()
        .filter(|i| i.status == "running")
        .collect();
    match running.len() {
        1 => {
            let inst = &running[0];
            Ok(ResolvedSession {
                name: if inst.name.is_empty() {
                    inst.uuid.clone()
                } else {
                    inst.name.clone()
                },
                instance_uuid: inst.uuid.clone(),
                from_local_cache: false,
                default_remote_dir: default_remote_dir_string(),
            })
        }
        0 => Err(AppError::NotFound(
            "No active sessions found. Run `autodl new` to create an instance or specify `-s <instance_uuid>`."
                .into(),
        )),
        _ => {
            let names: Vec<String> = running
                .iter()
                .map(|i| format!("{} ({})", i.name, i.uuid))
                .collect();
            Err(AppError::AmbiguousSession(format!(
                "Multiple running remote instances found: {}. Specify one with -s/--session.",
                names.join(", ")
            )))
        }
    }
}

/// Generate a random short session name like `run-a1b2c3` or `s-a1b2c3`.
pub fn generate_session_name(prefix: &str) -> String {
    let mut rng = rand::thread_rng();
    let val: u32 = rng.gen_range(0..0x0100_0000);
    format!("{prefix}-{val:06x}")
}

/// Resolve configuration directory (`--config` > `AUTODL_CONFIG_DIR` > OS standard config dir).
pub fn resolve_config_dir(cli_override: Option<&Path>) -> Result<PathBuf, AppError> {
    if let Some(p) = cli_override {
        return Ok(p.to_path_buf());
    }
    if let Ok(env_dir) = std::env::var("AUTODL_CONFIG_DIR") {
        let trimmed = env_dir.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Some(proj) = ProjectDirs::from("com", "autodl", "autodl-cli") {
        return Ok(proj.config_dir().to_path_buf());
    }
    if let Some(base) = directories::BaseDirs::new() {
        return Ok(base.home_dir().join(".config").join("autodl-cli"));
    }
    Err(AppError::Other(
        "Unable to determine configuration directory; pass --config <DIR>".into(),
    ))
}

pub fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T, AppError> {
    match fs::read(path) {
        Ok(bytes) => {
            if bytes.is_empty() {
                return Ok(T::default());
            }
            serde_json::from_slice(&bytes).map_err(|e| {
                AppError::Other(format!(
                    "Failed to parse JSON state file '{}': {e}",
                    path.display()
                ))
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(AppError::Io(e)),
    }
}

pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| AppError::Other(format!("Failed to serialize JSON: {e}")))?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let rand_suffix: u32 = rand::thread_rng().r#gen();
    let tmp_name = format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("state"),
        std::process::id(),
        rand_suffix
    );
    let tmp_path = parent.join(tmp_name);
    fs::write(&tmp_path, &json)?;
    set_private_permissions(&tmp_path);

    // On Windows, fs::rename replaces existing files in modern std, but remove first if needed.
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp_path, path)?;
    set_private_permissions(path);
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}
