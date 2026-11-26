pub mod store;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::api::AutoDlClient;
use crate::error::AppError;
use crate::output::mask_secret;
use crate::state::{SessionStore, read_json_or_default, write_json_atomic};

use self::store::{CredentialStore, default_credential_store};

pub const DEFAULT_PROFILE_NAME: &str = "default";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProfileRegistry {
    pub active_profile: Option<String>,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileMeta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileMeta {
    pub masked_token: String,
    pub updated_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Env,
    Keyring,
}

#[derive(Debug, Clone)]
pub struct ResolvedAuth {
    pub profile: String,
    pub token: String,
    pub source: TokenSource,
    pub api_base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogoutSummary {
    pub cleared_profiles: Vec<String>,
    pub credentials_deleted: usize,
    pub sessions_cleared: usize,
    pub backend: &'static str,
    pub next_active_profile: Option<String>,
}

pub struct AuthManager {
    profiles_path: PathBuf,
    store: Box<dyn CredentialStore>,
}

impl AuthManager {
    pub fn new(config_dir: &Path) -> Self {
        Self::with_store(config_dir, default_credential_store())
    }

    pub fn with_store(config_dir: &Path, store: Box<dyn CredentialStore>) -> Self {
        Self {
            profiles_path: config_dir.join("profiles.json"),
            store,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.store.backend_name()
    }

    pub fn profiles_path(&self) -> &Path {
        &self.profiles_path
    }

    pub fn load_registry(&self) -> Result<ProfileRegistry, AppError> {
        read_json_or_default(&self.profiles_path)
    }

    pub fn save_registry(&self, registry: &ProfileRegistry) -> Result<(), AppError> {
        write_json_atomic(&self.profiles_path, registry)
    }

    /// Resolve the profile name to use:
    /// explicit `--profile` > `AUTODL_PROFILE` env > active profile in `profiles.json` > `"default"`.
    pub fn resolve_profile_name(&self, cli_profile: Option<&str>) -> Result<String, AppError> {
        if let Some(p) = cli_profile.map(str::trim).filter(|s| !s.is_empty()) {
            validate_profile_name(p)?;
            return Ok(p.to_string());
        }
        if let Ok(env_p) = std::env::var("AUTODL_PROFILE") {
            let trimmed = env_p.trim();
            if !trimmed.is_empty() {
                validate_profile_name(trimmed)?;
                return Ok(trimmed.to_string());
            }
        }
        let reg = self.load_registry()?;
        Ok(reg
            .active_profile
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string()))
    }

    /// Resolve token for API calls. Checks `AUTODL_TOKEN` env first, then the credential store.
    pub fn resolve_auth(&self, cli_profile: Option<&str>) -> Result<ResolvedAuth, AppError> {
        let profile = self.resolve_profile_name(cli_profile)?;
        let reg = self.load_registry()?;
        let api_base_url = reg
            .profiles
            .get(&profile)
            .and_then(|m| m.api_base_url.clone());

        if let Ok(env_token) = std::env::var("AUTODL_TOKEN") {
            let trimmed = env_token.trim();
            if !trimmed.is_empty() {
                return Ok(ResolvedAuth {
                    profile,
                    token: trimmed.to_string(),
                    source: TokenSource::Env,
                    api_base_url,
                });
            }
        }

        match self.store.get(&profile)? {
            Some(token) if !token.trim().is_empty() => Ok(ResolvedAuth {
                profile,
                token: token.trim().to_string(),
                source: TokenSource::Keyring,
                api_base_url,
            }),
            _ => Err(AppError::Auth(format!(
                "Not logged in for profile '{profile}'. Run `autodl auth login` or set AUTODL_TOKEN."
            ))),
        }
    }

    /// Store token in OS credential manager, update `profiles.json`, and activate profile.
    pub async fn login(
        &self,
        profile: &str,
        token: &str,
        api_base_url: Option<String>,
        verify: bool,
        verbose: bool,
    ) -> Result<ProfileMeta, AppError> {
        validate_profile_name(profile)?;
        let clean_token = token.trim();
        if clean_token.is_empty() {
            return Err(AppError::Validation("Token cannot be empty".into()));
        }

        if verify {
            let client = AutoDlClient::new(clean_token.to_string(), api_base_url.as_deref())?
                .with_verbose(verbose);
            client.wallet_balance().await.map_err(|e| match e {
                AppError::Api {
                    code,
                    msg,
                    request_id,
                    ..
                } => {
                    let rid = request_id
                        .as_deref()
                        .map(|r| format!(" [request_id={r}]"))
                        .unwrap_or_default();
                    AppError::Auth(format!(
                        "Token verification failed ({code}): {msg}{rid}"
                    ))
                }
                other => other,
            })?;
        }

        self.store.set(profile, clean_token)?;

        let mut reg = self.load_registry()?;
        let meta = ProfileMeta {
            masked_token: mask_secret(clean_token),
            updated_at: Utc::now(),
            api_base_url,
        };
        reg.profiles.insert(profile.to_string(), meta.clone());
        reg.active_profile = Some(profile.to_string());
        self.save_registry(&reg)?;

        Ok(meta)
    }

    /// Switch the active profile (must already exist in `profiles.json` or credential store).
    pub fn switch_profile(&self, profile: &str) -> Result<(), AppError> {
        validate_profile_name(profile)?;
        let mut reg = self.load_registry()?;
        let in_store = self.store.get(profile)?.is_some();
        if !reg.profiles.contains_key(profile) && !in_store {
            return Err(AppError::NotFound(format!(
                "Profile '{profile}' does not exist. Log in first with `autodl auth login --profile {profile}`."
            )));
        }
        reg.active_profile = Some(profile.to_string());
        self.save_registry(&reg)?;
        Ok(())
    }

    /// Check whether a token exists in the credential store for the given profile.
    pub fn has_stored_token(&self, profile: &str) -> Result<bool, AppError> {
        Ok(self.store.get(profile)?.is_some())
    }

    /// Complete account logout:
    /// 1. Delete token(s) from OS Credential Manager (`keyring`).
    /// 2. Remove profile metadata from `profiles.json` and update `active_profile`.
    /// 3. Clear cached local sessions for logged-out profile(s) unless `keep_sessions` is true.
    pub fn logout(
        &self,
        cli_profile: Option<&str>,
        all: bool,
        keep_sessions: bool,
        session_store: &SessionStore,
    ) -> Result<LogoutSummary, AppError> {
        let mut reg = self.load_registry()?;
        let targets: Vec<String> = if all {
            let mut set: Vec<String> = reg.profiles.keys().cloned().collect();
            if let Some(active) = &reg.active_profile
                && !set.contains(active)
            {
                set.push(active.clone());
            }
            if set.is_empty() {
                set.push(DEFAULT_PROFILE_NAME.to_string());
            }
            set
        } else {
            vec![self.resolve_profile_name(cli_profile)?]
        };

        let mut cleared_profiles = Vec::new();
        let mut credentials_deleted = 0usize;
        let mut sessions_cleared = 0usize;

        for profile in targets {
            let deleted = self.store.delete(&profile)?;
            if deleted {
                credentials_deleted += 1;
            }
            let removed_meta = reg.profiles.remove(&profile).is_some();
            if reg.active_profile.as_deref() == Some(profile.as_str()) {
                reg.active_profile = None;
            }
            if !keep_sessions {
                sessions_cleared += session_store.clear_profile(&profile)?;
            }
            if deleted || removed_meta || !keep_sessions {
                cleared_profiles.push(profile);
            }
        }

        if reg.active_profile.is_none() {
            reg.active_profile = reg.profiles.keys().next().cloned();
        }
        self.save_registry(&reg)?;

        Ok(LogoutSummary {
            cleared_profiles,
            credentials_deleted,
            sessions_cleared,
            backend: self.store.backend_name(),
            next_active_profile: reg.active_profile,
        })
    }
}

pub fn validate_profile_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() || name.len() > 64 {
        return Err(AppError::Validation(
            "Profile name must be 1..=64 characters".into(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AppError::Validation(format!(
            "Invalid profile name '{name}': only ASCII letters, digits, '-' and '_' are allowed"
        )));
    }
    Ok(())
}
