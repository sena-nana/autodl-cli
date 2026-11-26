use std::process;

use clap::Parser;

use autodl_cli::cli::{Cli, run_cli};
use autodl_cli::error::AppError;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json_mode = cli.json;

    if let Err(err) = run_cli(cli).await {
        let code = err.exit_code();
        if !matches!(err, AppError::ReportedExit(_)) {
            if json_mode {
                if let Ok(text) = serde_json::to_string_pretty(&err.to_json()) {
                    println!("{text}");
                }
            } else {
                eprintln!("error: {err}");
            }
        }
        process::exit(code);
    }
}
