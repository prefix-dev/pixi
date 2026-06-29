//! Thin wrapper around rattler's auth CLI.

use clap::{Parser, Subcommand};
use miette::IntoDiagnostic;

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    subcommand: Option<AuthCommand>,
}

#[derive(Subcommand, Debug)]
enum AuthCommand {
    /// Show stored authentication entries and non-secret token metadata
    Status {
        #[arg(value_name = "HOST")]
        host: Option<String>,
    },
    /// Store authentication information for a given host
    Login {
        #[arg(value_name = "HOST")]
        host: String,
    },
    /// Remove authentication information for a given host
    Logout {
        #[arg(value_name = "HOST")]
        host: String,
    },
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let mut argv: Vec<String> = vec!["auth".to_string()];
    match args.subcommand {
        Some(AuthCommand::Status { host }) => {
            argv.push("status".to_string());
            argv.extend(host);
        }
        Some(AuthCommand::Login { host }) => {
            argv.extend(["login".to_string(), host]);
        }
        Some(AuthCommand::Logout { host }) => {
            argv.extend(["logout".to_string(), host]);
        }
        None => argv.push("--help".to_string()),
    }
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    rattler::cli::auth::execute(
        rattler::cli::auth::Args::try_parse_from(&argv).into_diagnostic()?,
    )
    .await
    .into_diagnostic()
}
