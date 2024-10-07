use crate::global::Project;
use clap::Parser;
use miette::IntoDiagnostic;
use pixi_config::{Config, ConfigCli};

/// Edit the global manifest file
///
/// Opens your default editor to edit the global manifest file.
#[derive(Parser, Debug)]
#[clap(verbatim_doc_comment)]
pub struct Args {
    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project = Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
        #[cfg(not(target_os = "windows"))]
        {
            "nano".to_string()
        }
        #[cfg(target_os = "windows")]
        {
            "notepad".to_string()
        }
    });

    let mut child = std::process::Command::new(editor.as_str())
        .arg(project.manifest.path)
        .spawn()
        .into_diagnostic()?;
    child.wait().into_diagnostic()?;

    Ok(())
}
