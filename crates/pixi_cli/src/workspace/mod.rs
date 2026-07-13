use clap::Parser;
use itertools::Itertools;
use pixi_manifest::Feature;

use crate::cli_config::WorkspaceConfig;

pub mod channel;
pub mod description;
pub mod environment;
pub mod export;
pub mod feature;
pub mod name;
pub mod platform;
pub mod register;
pub mod requires_pixi;
pub mod version;

#[derive(Debug, Parser)]
pub enum Command {
    Channel(channel::Args),
    Description(description::Args),
    Platform(platform::Args),
    Version(version::Args),
    Environment(environment::Args),
    Feature(feature::Args),
    Export(export::Args),
    Name(name::Args),
    Register(register::Args),
    RequiresPixi(requires_pixi::Args),
}

/// Modify the workspace configuration file through the command line.
#[derive(Debug, Parser)]
pub struct Args {
    #[command(subcommand)]
    command: Command,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,
}

pub async fn execute(cmd: Args) -> miette::Result<()> {
    match cmd.command {
        Command::Channel(args) => channel::execute(args).await?,
        Command::Description(args) => description::execute(args).await?,
        Command::Platform(args) => platform::execute(args).await?,
        Command::Version(args) => version::execute(args).await?,
        Command::Environment(args) => environment::execute(args).await?,
        Command::Feature(args) => feature::execute(args).await?,
        Command::Export(cmd) => export::execute(cmd).await?,
        Command::Name(args) => name::execute(args).await?,
        Command::Register(args) => register::execute(args).await?,
        Command::RequiresPixi(args) => requires_pixi::execute(args).await?,
    };
    Ok(())
}

/// Indented detail lines describing a feature's content (dependencies,
/// pypi-dependencies, tasks), shared by the `feature list` and
/// `environment list` renderings.
fn feature_detail_lines(feature: &Feature) -> Vec<String> {
    let deps: Vec<_> = feature
        .dependencies(pixi_manifest::SpecType::Run, None)
        .map(|d| d.names().map(|n| n.as_normalized().to_string()).collect())
        .unwrap_or_default();

    let pypi_deps: Vec<_> = feature
        .pypi_dependencies(None)
        .map(|d| d.names().map(|n| n.as_source().to_string()).collect())
        .unwrap_or_default();

    let tasks: Vec<_> = feature
        .targets
        .default()
        .tasks
        .keys()
        .map(|k| k.as_str().to_string())
        .collect();

    let mut details = Vec::new();

    if !deps.is_empty() {
        let deps = deps.iter().map(|d| console::style(d).green()).join(", ");
        details.push(format!("    dependencies: {deps}"));
    }
    if !pypi_deps.is_empty() {
        let deps = pypi_deps
            .iter()
            .map(|d| console::style(d).blue())
            .join(", ");
        details.push(format!("    pypi-dependencies: {deps}"));
    }
    if !tasks.is_empty() {
        details.push(format!("    tasks: {}", tasks.join(", ")));
    }

    details
}
