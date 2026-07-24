use std::path::PathBuf;

use clap::Parser;
use miette::IntoDiagnostic;
use pixi_api::Interface;
use pixi_config::Config;
use pixi_manifest::script::ScriptManifest;
use rattler_conda_types::NamedChannelOrUrl;

use crate::cli_interface::CliInterface;

#[derive(Debug, Parser)]
pub struct Args {
    /// Script to initialize.
    pub path: PathBuf,

    /// Channel to use when resolving conda dependencies.
    #[arg(short, long = "channel", value_name = "CHANNEL")]
    pub channels: Option<Vec<NamedChannelOrUrl>>,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let path = std::path::absolute(args.path).into_diagnostic()?;
    let parent = path
        .parent()
        .expect("an absolute script path always has a parent");
    let config = Config::load(parent);
    let channels = args
        .channels
        .unwrap_or_else(|| config.default_channels())
        .into_iter()
        .map(|channel| channel.to_string())
        .collect::<Vec<_>>();
    let script = ScriptManifest::initialize(&path, &channels)?;

    CliInterface::default()
        .success(&format!(
            "Initialized script at {}",
            script.path().display()
        ))
        .await;
    Ok(())
}
