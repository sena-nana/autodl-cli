use std::io::{self, IsTerminal, Read};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::api::AutoDlClient;
use crate::auth::DEFAULT_PROFILE_NAME;
use crate::error::AppError;
use crate::output::{format_cny_milli, make_table, mask_secret};

use super::CliContext;

#[derive(Debug, Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommand,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Store an AutoDL developer token in the OS credential manager
    Login(LoginArgs),
    /// Show active authentication status and credential backend
    Status,
    /// List configured profiles
    List,
    /// Switch the active profile
    Switch(SwitchArgs),
    /// Sign out and remove stored token(s) from the OS credential manager
    Logout(LogoutArgs),
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// AutoDL developer token (or pass via --token-stdin / interactive hidden prompt)
    #[arg(long)]
    pub token: Option<String>,
    /// Read token from stdin (recommended for scripts/agents to avoid shell history)
    #[arg(long)]
    pub token_stdin: bool,
    /// Custom API base URL for this profile (default: https://api.autodl.com)
    #[arg(long)]
    pub api_base: Option<String>,
    /// Skip calling wallet/balance to verify the token before saving
    #[arg(long)]
    pub no_verify: bool,
}

#[derive(Debug, Args)]
pub struct SwitchArgs {
    /// Profile name to activate
    pub profile: String,
}

#[derive(Debug, Args, Default)]
pub struct LogoutArgs {
    /// Log out of all saved profiles and clear all cached local sessions
    #[arg(long)]
    pub all: bool,
    /// Keep local cached session metadata in sessions.json (credentials are still deleted)
    #[arg(long)]
    pub keep_sessions: bool,
}

#[derive(Debug, Serialize)]
struct AuthStatusReport {
    ok: bool,
    logged_in: bool,
    profile: String,
    backend: &'static str,
    source: Option<String>,
    masked_token: Option<String>,
    api_base_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProfileListItem {
    profile: String,
    active: bool,
    masked_token: String,
    has_credential: bool,
    updated_at: String,
    api_base_url: Option<String>,
}

pub async fn handle_auth(ctx: &CliContext, args: AuthArgs) -> Result<(), AppError> {
    match args.command {
        AuthCommand::Login(login_args) => handle_login(ctx, login_args).await,
        AuthCommand::Status => handle_status(ctx).await,
        AuthCommand::List => handle_list(ctx).await,
        AuthCommand::Switch(switch_args) => handle_switch(ctx, switch_args).await,
        AuthCommand::Logout(logout_args) => handle_logout(ctx, logout_args).await,
    }
}

pub async fn handle_login(ctx: &CliContext, args: LoginArgs) -> Result<(), AppError> {
    let auth_mgr = ctx.auth_mgr();
    let profile = ctx
        .cli_profile
        .clone()
        .or_else(|| std::env::var("AUTODL_PROFILE").ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| DEFAULT_PROFILE_NAME.to_string());

    let token = if let Some(t) = args.token {
        t
    } else if args.token_stdin {
        let mut buf = String::new();
        io::stdin().read_to_string(&mut buf)?;
        buf.trim().to_string()
    } else if io::stdin().is_terminal() {
        rpassword::prompt_password("Enter AutoDL Developer Token: ")
            .map_err(|e| AppError::Io(io::Error::other(format!("Failed reading token: {e}"))))?
    } else {
        return Err(AppError::Validation(
            "Provide --token <TOKEN> or --token-stdin when stdin is not a TTY".into(),
        ));
    };

    if !args.no_verify {
        ctx.out.status("Verifying token with AutoDL API...");
    }

    let meta = auth_mgr
        .login(
            &profile,
            &token,
            args.api_base.clone(),
            !args.no_verify,
            ctx.out.verbose,
        )
        .await?;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "profile": profile,
            "backend": auth_mgr.backend_name(),
            "masked_token": meta.masked_token,
            "verified": !args.no_verify,
            "api_base_url": meta.api_base_url,
        }))?;
    } else {
        ctx.out.println(&format!(
            "Logged in as profile '{}' (token {}, stored in {}).",
            profile,
            meta.masked_token,
            auth_mgr.backend_name()
        ));
    }
    Ok(())
}

pub async fn handle_status(ctx: &CliContext) -> Result<(), AppError> {
    let auth_mgr = ctx.auth_mgr();
    let profile = auth_mgr.resolve_profile_name(ctx.cli_profile.as_deref())?;
    let reg = auth_mgr.load_registry()?;
    let meta = reg.profiles.get(&profile);

    let resolved = auth_mgr.resolve_auth(ctx.cli_profile.as_deref());
    let report = match resolved {
        Ok(auth) => AuthStatusReport {
            ok: true,
            logged_in: true,
            profile: auth.profile,
            backend: auth_mgr.backend_name(),
            source: Some(match auth.source {
                crate::auth::TokenSource::Env => "env".to_string(),
                crate::auth::TokenSource::Keyring => "keyring".to_string(),
            }),
            masked_token: Some(mask_secret(&auth.token)),
            api_base_url: auth.api_base_url,
        },
        Err(_) => AuthStatusReport {
            ok: true,
            logged_in: false,
            profile,
            backend: auth_mgr.backend_name(),
            source: None,
            masked_token: meta.map(|m| m.masked_token.clone()),
            api_base_url: meta.and_then(|m| m.api_base_url.clone()),
        },
    };

    if ctx.out.json {
        ctx.out.emit_json(&report)?;
    } else if report.logged_in {
        ctx.out.println(&format!(
            "Profile: {} | Status: logged in ({}) | Token: {} | Backend: {}",
            report.profile,
            report.source.as_deref().unwrap_or("unknown"),
            report.masked_token.as_deref().unwrap_or("****"),
            report.backend
        ));
    } else {
        ctx.out.println(&format!(
            "Profile: {} | Status: not logged in | Backend: {}",
            report.profile, report.backend
        ));
    }
    Ok(())
}

pub async fn handle_list(ctx: &CliContext) -> Result<(), AppError> {
    let auth_mgr = ctx.auth_mgr();
    let reg = auth_mgr.load_registry()?;
    let active = auth_mgr.resolve_profile_name(ctx.cli_profile.as_deref())?;

    let mut items = Vec::with_capacity(reg.profiles.len());
    for (name, meta) in &reg.profiles {
        let has_cred = auth_mgr.has_stored_token(name).unwrap_or(false);
        items.push(ProfileListItem {
            profile: name.clone(),
            active: *name == active,
            masked_token: meta.masked_token.clone(),
            has_credential: has_cred,
            updated_at: meta.updated_at.to_rfc3339(),
            api_base_url: meta.api_base_url.clone(),
        });
    }

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "active_profile": active,
            "backend": auth_mgr.backend_name(),
            "profiles": items,
        }))?;
        return Ok(());
    }

    if items.is_empty() {
        ctx.out
            .println("No profiles configured. Run `autodl auth login` to add one.");
        return Ok(());
    }

    let mut table = make_table(&["Active", "Profile", "Token", "Keyring", "Updated"]);
    for item in items {
        table.add_row(vec![
            if item.active { "*" } else { "" },
            &item.profile,
            &item.masked_token,
            if item.has_credential { "yes" } else { "missing" },
            &item.updated_at,
        ]);
    }
    println!("{table}");
    Ok(())
}

pub async fn handle_switch(ctx: &CliContext, args: SwitchArgs) -> Result<(), AppError> {
    let auth_mgr = ctx.auth_mgr();
    auth_mgr.switch_profile(&args.profile)?;
    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "active_profile": args.profile,
        }))?;
    } else {
        ctx.out
            .println(&format!("Switched active profile to '{}'.", args.profile));
    }
    Ok(())
}

pub async fn handle_logout(ctx: &CliContext, args: LogoutArgs) -> Result<(), AppError> {
    let auth_mgr = ctx.auth_mgr();
    let store = ctx.session_store();

    // Warn before clearing if there are tracked sessions that might still be running remotely.
    if !args.keep_sessions
        && let Ok(auth) = auth_mgr.resolve_auth(ctx.cli_profile.as_deref())
    {
        let tracked = store.list_for_profile(&auth.profile).unwrap_or_default();
        if !tracked.is_empty()
            && let Ok(client) = AutoDlClient::new(auth.token, auth.api_base_url.as_deref())
            && let Ok(remote) = client.list_all_pro_instances().await
        {
            let running_names: Vec<String> = tracked
                .iter()
                .filter(|t| {
                    remote
                        .iter()
                        .any(|r| r.uuid == t.instance_uuid && r.status == "running")
                })
                .map(|t| format!("{} ({})", t.name, t.instance_uuid))
                .collect();
            if !running_names.is_empty() {
                ctx.out.warn(&format!(
                    "Tracked instances still running on AutoDL: {}. Logging out does NOT release cloud instances.",
                    running_names.join(", ")
                ));
            }
        }
    }

    let summary = auth_mgr.logout(
        ctx.cli_profile.as_deref(),
        args.all,
        args.keep_sessions,
        &store,
    )?;

    if std::env::var("AUTODL_TOKEN")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        ctx.out.warn(
            "AUTODL_TOKEN is set in your environment and will still override stored credentials until unset.",
        );
    }

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "cleared_profiles": summary.cleared_profiles,
            "credentials_deleted": summary.credentials_deleted,
            "sessions_cleared": summary.sessions_cleared,
            "backend": summary.backend,
            "next_active_profile": summary.next_active_profile,
        }))?;
    } else {
        ctx.out.println(&format!(
            "Logged out profiles: [{}] | Removed {} credential(s) from {} | Cleared {} local session(s).",
            summary.cleared_profiles.join(", "),
            summary.credentials_deleted,
            summary.backend,
            summary.sessions_cleared
        ));
    }
    Ok(())
}

pub async fn handle_whoami(ctx: &CliContext) -> Result<(), AppError> {
    let auth_mgr = ctx.auth_mgr();
    let auth = auth_mgr.resolve_auth(ctx.cli_profile.as_deref())?;
    let client = AutoDlClient::new(auth.token.clone(), auth.api_base_url.as_deref())?
        .with_verbose(ctx.out.verbose);
    let balance = client.wallet_balance().await?;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "profile": auth.profile,
            "source": match auth.source {
                crate::auth::TokenSource::Env => "env",
                crate::auth::TokenSource::Keyring => "keyring",
            },
            "backend": auth_mgr.backend_name(),
            "masked_token": mask_secret(&auth.token),
            "api_base_url": client.base_url(),
            "wallet": {
                "assets_milli": balance.assets,
                "assets_cny": balance.assets as f64 / 1000.0,
                "voucher_balance_milli": balance.voucher_balance,
                "voucher_balance_cny": balance.voucher_balance as f64 / 1000.0,
                "accumulate_milli": balance.accumulate,
                "accumulate_cny": balance.accumulate as f64 / 1000.0,
            }
        }))?;
    } else {
        ctx.out.println(&format!(
            "Profile: {} ({}) | Token: {} | Balance: {} (Voucher: {}, Total Spent: {})",
            auth.profile,
            match auth.source {
                crate::auth::TokenSource::Env => "env",
                crate::auth::TokenSource::Keyring => "keyring",
            },
            mask_secret(&auth.token),
            format_cny_milli(balance.assets),
            format_cny_milli(balance.voucher_balance),
            format_cny_milli(balance.accumulate),
        ));
    }
    Ok(())
}

pub async fn handle_usage(ctx: &CliContext) -> Result<(), AppError> {
    let (client, profile) = ctx.api_client()?;
    let bal = client.wallet_balance().await?;

    if ctx.out.json {
        ctx.out.emit_json(&serde_json::json!({
            "ok": true,
            "profile": profile,
            "assets_milli": bal.assets,
            "assets_cny": bal.assets as f64 / 1000.0,
            "voucher_balance_milli": bal.voucher_balance,
            "voucher_balance_cny": bal.voucher_balance as f64 / 1000.0,
            "accumulate_milli": bal.accumulate,
            "accumulate_cny": bal.accumulate as f64 / 1000.0,
        }))?;
    } else {
        let mut table = make_table(&["Metric", "CNY", "Raw (milli-CNY)"]);
        table.add_row(vec![
            "Available Balance",
            &format_cny_milli(bal.assets),
            &bal.assets.to_string(),
        ]);
        table.add_row(vec![
            "Voucher Balance",
            &format_cny_milli(bal.voucher_balance),
            &bal.voucher_balance.to_string(),
        ]);
        table.add_row(vec![
            "Cumulative Spend",
            &format_cny_milli(bal.accumulate),
            &bal.accumulate.to_string(),
        ]);
        println!("{table}");
    }
    Ok(())
}
