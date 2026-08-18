use std::{io::Write, path::PathBuf};

use clap::{
    Parser,
    builder::{PossibleValuesParser, TypedValueParser},
};
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::{WorkspaceLocator, WorkspaceLocatorError};
use pixi_manifest::KnownPreviewFeature;
use strum::VariantNames;

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

/// Commands to manage workspace preview features.
#[derive(Parser, Debug)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: Command,
}

#[derive(Parser, Debug)]
pub struct AddRemoveArgs {
    /// The preview feature(s) to add or remove, e.g. `pixi-build`.
    #[clap(required = true, num_args = 1.., value_parser = feature_parser(), value_name = "PREVIEW_FEATURE")]
    pub features: Vec<KnownPreviewFeature>,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Add preview feature(s) to the workspace.
    ///
    /// Example:
    /// `pixi workspace preview add pixi-build`
    #[clap(visible_alias = "a")]
    Add(AddRemoveArgs),
    /// List the enabled preview features.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove preview feature(s) from the workspace.
    ///
    /// Example:
    /// `pixi workspace preview remove pixi-build`
    #[clap(visible_alias = "rm")]
    Remove(AddRemoveArgs),
}

/// Only known preview features are accepted, and they tab-complete.
fn feature_parser() -> impl TypedValueParser<Value = KnownPreviewFeature> {
    PossibleValuesParser::new(KnownPreviewFeature::VARIANTS).map(|name| {
        name.parse()
            .expect("the possible values are known features")
    })
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let located = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate();

    let workspace = match located {
        Ok(workspace) => workspace,
        // The workspace may fail to load exactly because the feature is
        // missing, e.g. a `[package]` section without `pixi-build`, so `add`
        // falls back to editing the manifest directly.
        Err(WorkspaceLocatorError::Toml(err)) if matches!(args.command, Command::Add(_)) => {
            let Command::Add(add) = args.command else {
                unreachable!("the match guard checked the command")
            };
            return WorkspaceContext::add_preview_features_without_workspace(
                CliInterface {},
                PathBuf::from(err.source.name()),
                add.features,
            )
            .await;
        }
        Err(err) => return Err(err.into()),
    };

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace);

    match args.command {
        Command::Add(args) => workspace_ctx.add_preview_features(args.features).await?,
        Command::List => {
            let mut stdout = std::io::stdout();
            for feature in workspace_ctx.preview_features().await {
                writeln!(stdout, "{feature}")
                    .inspect_err(|e| {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            std::process::exit(0);
                        }
                    })
                    .into_diagnostic()?;
            }
        }
        Command::Remove(args) => workspace_ctx.remove_preview_features(args.features).await?,
    }

    Ok(())
}
