pub mod auth;
pub mod deploy;
pub mod exec;
pub mod files;
pub mod instance;
pub mod storage;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::api::AutoDlClient;
use crate::auth::AuthManager;
use crate::error::AppError;
use crate::output::Output;
use crate::state::{SessionStore, resolve_config_dir};

#[derive(Debug, Parser)]
#[command(
    name = "autodl",
    version,
    about = "Terminal-first, AI-agent-friendly CLI for AutoDL GPU clouds (inspired by google-colab-cli)",
    long_about = "Provision AutoDL GPU instances, store developer tokens in your OS credential manager, sync code, run scripts with automatic teardown, and inspect elastic deployments."
)]
pub struct Cli {
    /// Emit machine-readable JSON output to stdout (logs & warnings go to stderr)
    #[arg(long, global = true, env = "AUTODL_JSON")]
    pub json: bool,

    /// Credential and session profile name
    #[arg(long, global = true, env = "AUTODL_PROFILE")]
    pub profile: Option<String>,

    /// Override configuration/state directory
    #[arg(long, global = true, env = "AUTODL_CONFIG_DIR")]
    pub config: Option<PathBuf>,

    /// Suppress non-essential informational stderr output
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,

    /// Print debug HTTP/diagnostic logs to stderr
    #[arg(short = 'v', long, visible_alias = "logtostderr", global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Manage AutoDL developer tokens in the OS credential manager
    Auth(auth::AuthArgs),
    /// Sign out and remove stored credentials + cached sessions (alias for `auth logout`)
    Logout(auth::LogoutArgs),
    /// Verify active credentials and show account info & balance
    Whoami,
    /// Show wallet balance, vouchers, and cumulative spend
    #[command(visible_alias = "balance")]
    Usage,

    /// Create and boot a new AutoDL Pro GPU instance
    New(instance::NewArgs),
    /// List local sessions and remote AutoDL Pro instances
    Sessions,
    /// Show detailed status, hardware, price, SSH, and resource usage for a session
    Status(instance::SessionTargetArgs),
    /// Power on a stopped AutoDL Pro instance (with optional boot command)
    PowerOn(instance::PowerOnArgs),
    /// Power off a running AutoDL Pro instance (preserves container & disk)
    PowerOff(instance::PowerOffArgs),
    /// Stop and permanently release an AutoDL Pro instance
    #[command(visible_alias = "release")]
    Stop(instance::StopArgs),
    /// Print or open JupyterLab & service URLs for an instance
    Url(instance::UrlArgs),
    /// Fetch `/var/log/autodl-start.log` and boot exit status from an instance
    BootLog(instance::BootLogArgs),

    /// Execute a local script, command, or piped stdin on a running instance
    Exec(exec::ExecArgs),
    /// Ephemeral auto-release runner: create GPU -> sync/upload -> run -> download -> release
    Run(Box<exec::RunArgs>),
    /// Open an interactive SSH shell or run a one-shot remote command (`-- CMD`)
    Ssh(exec::SshArgs),
    /// Install Python packages on an instance (`uv pip` with `pip` fallback)
    Install(exec::InstallArgs),

    /// Upload a local file or directory to an instance
    Upload(files::UploadArgs),
    /// Fast `.gitignore`-aware code sync (`.tar.gz` stream over SSH)
    Sync(files::SyncArgs),
    /// Download a remote file or directory (`-r`) from an instance
    Download(files::DownloadArgs),
    /// List remote files via SFTP
    Ls(files::LsArgs),
    /// Remove a remote file or directory (`-r`)
    Rm(files::RmArgs),

    /// List or save private container images
    Images(deploy::ImagesArgs),
    /// Mount or unmount exclusive NFS in a data center
    Nfs(deploy::NfsArgs),
    /// Manage AutoDL shared file storage (`/root/autodl-fs`) on a running instance
    Fs(storage::FsArgs),
    /// Inspect disk capacity, mount points, and inode usage on a running instance
    Disk(storage::DiskArgs),
    /// Query real-time GPU availability by region
    Stock(deploy::StockArgs),
    /// List built-in GPU spec IDs, base image UUIDs, and region codes
    Catalog(deploy::CatalogArgs),
    /// Manage AutoDL Elastic Deployments (enterprise API)
    Deploy(deploy::DeployArgs),

    /// Print CLI version information
    Version,
}

pub struct CliContext {
    pub config_dir: PathBuf,
    pub cli_profile: Option<String>,
    pub out: Output,
}

impl CliContext {
    pub fn from_cli(cli: &Cli) -> Result<Self, AppError> {
        let config_dir = resolve_config_dir(cli.config.as_deref())?;
        Ok(Self {
            config_dir,
            cli_profile: cli.profile.clone(),
            out: Output::new(cli.json, cli.quiet, cli.verbose),
        })
    }

    pub fn auth_mgr(&self) -> AuthManager {
        AuthManager::new(&self.config_dir)
    }

    pub fn session_store(&self) -> SessionStore {
        SessionStore::new(&self.config_dir)
    }

    pub fn api_client(&self) -> Result<(AutoDlClient, String), AppError> {
        let auth = self
            .auth_mgr()
            .resolve_auth(self.cli_profile.as_deref())?;
        let client = AutoDlClient::new(auth.token, auth.api_base_url.as_deref())?
            .with_verbose(self.out.verbose);
        Ok((client, auth.profile))
    }
}

pub async fn run_cli(cli: Cli) -> Result<(), AppError> {
    let ctx = CliContext::from_cli(&cli)?;
    match cli.command {
        Commands::Auth(args) => auth::handle_auth(&ctx, args).await,
        Commands::Logout(args) => auth::handle_logout(&ctx, args).await,
        Commands::Whoami => auth::handle_whoami(&ctx).await,
        Commands::Usage => auth::handle_usage(&ctx).await,

        Commands::New(args) => instance::handle_new(&ctx, args).await,
        Commands::Sessions => instance::handle_sessions(&ctx).await,
        Commands::Status(args) => instance::handle_status(&ctx, args).await,
        Commands::PowerOn(args) => instance::handle_power_on(&ctx, args).await,
        Commands::PowerOff(args) => instance::handle_power_off(&ctx, args).await,
        Commands::Stop(args) => instance::handle_stop(&ctx, args).await,
        Commands::Url(args) => instance::handle_url(&ctx, args).await,
        Commands::BootLog(args) => instance::handle_boot_log(&ctx, args).await,

        Commands::Exec(args) => exec::handle_exec(&ctx, args).await,
        Commands::Run(args) => exec::handle_run(&ctx, *args).await,
        Commands::Ssh(args) => exec::handle_ssh(&ctx, args).await,
        Commands::Install(args) => exec::handle_install(&ctx, args).await,

        Commands::Upload(args) => files::handle_upload(&ctx, args).await,
        Commands::Sync(args) => files::handle_sync(&ctx, args).await,
        Commands::Download(args) => files::handle_download(&ctx, args).await,
        Commands::Ls(args) => files::handle_ls(&ctx, args).await,
        Commands::Rm(args) => files::handle_rm(&ctx, args).await,

        Commands::Images(args) => deploy::handle_images(&ctx, args).await,
        Commands::Nfs(args) => deploy::handle_nfs(&ctx, args).await,
        Commands::Fs(args) => storage::handle_fs(&ctx, args).await,
        Commands::Disk(args) => storage::handle_disk(&ctx, args).await,
        Commands::Stock(args) => deploy::handle_stock(&ctx, args).await,
        Commands::Catalog(args) => deploy::handle_catalog(&ctx, args),
        Commands::Deploy(args) => deploy::handle_deploy(&ctx, args).await,

        Commands::Version => {
            if ctx.out.json {
                ctx.out.emit_json(&serde_json::json!({
                    "ok": true,
                    "name": env!("CARGO_PKG_NAME"),
                    "version": env!("CARGO_PKG_VERSION"),
                }))?;
            } else {
                println!("autodl-cli {}", env!("CARGO_PKG_VERSION"));
            }
            Ok(())
        }
    }
}
