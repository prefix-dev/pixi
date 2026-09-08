use std::{io::Write, path::PathBuf};

use clap::{
    Parser,
    builder::{PossibleValuesParser, TypedValueParser},
};
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_core::{Workspace, WorkspaceLocator, WorkspaceLocatorError};
use pixi_manifest::KnownPreviewFlag;
use strum::VariantNames;

use crate::{
    cli_config::WorkspaceConfig,
    cli_interface::{CliInterface, cli_context},
};

/// Commands to manage workspace preview flags.
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
#[clap(arg_required_else_help = true)]
pub struct AddRemoveArgs {
    /// The preview flag(s) to add or remove, e.g. `pixi-build`.
    #[clap(required = true, num_args = 1.., value_parser = flag_parser(), value_name = "PREVIEW_FLAG")]
    pub flags: Vec<KnownPreviewFlag>,
}

#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct RemoveArgs {
    #[clap(flatten)]
    pub args: AddRemoveArgs,

    /// Remove the flag(s) even when the manifest no longer loads without
    /// them.
    #[clap(long)]
    pub force: bool,
}

#[derive(Parser, Debug)]
pub enum Command {
    /// Add preview flag(s) to the workspace.
    ///
    /// Example:
    /// `pixi workspace preview add pixi-build`
    #[clap(visible_alias = "a")]
    Add(AddRemoveArgs),
    /// List the enabled preview flags.
    #[clap(visible_alias = "ls")]
    List,
    /// Remove preview flag(s) from the workspace.
    ///
    /// Example:
    /// `pixi workspace preview remove pixi-build`
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),
}

/// Only known preview flags are accepted, and they tab-complete.
///
/// The natural way would be to derive `clap::ValueEnum` on
/// [`KnownPreviewFlag`], but it lives in `pixi_manifest`, which doesn't
/// depend on clap, and the orphan rule prevents implementing clap's trait
/// for it here. Instead its strum derives provide the same two pieces:
/// the flag names (`VariantNames::VARIANTS`) for clap to validate,
/// complete, and list in help, and the conversion back (`FromStr`), which
/// can't fail since the input is one of those names.
fn flag_parser() -> impl TypedValueParser<Value = KnownPreviewFlag> {
    PossibleValuesParser::new(<KnownPreviewFlag as VariantNames>::VARIANTS)
        .map(|name| name.parse().expect("the possible values are known flags"))
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let located = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate();

    match args.command {
        Command::Add(add) => {
            WorkspaceContext::add_preview_flags(CliInterface {}, manifest_path(located)?, add.flags)
                .await
        }
        Command::Remove(remove) => {
            WorkspaceContext::remove_preview_flags(
                CliInterface {},
                manifest_path(located)?,
                remove.args.flags,
                remove.force,
            )
            .await
        }
        Command::List => {
            let workspace_ctx = cli_context(located?);
            let mut stdout = std::io::stdout();
            for flag in workspace_ctx.preview_flags().await {
                writeln!(stdout, "{flag}")
                    .inspect_err(|e| {
                        if e.kind() == std::io::ErrorKind::BrokenPipe {
                            std::process::exit(0);
                        }
                    })
                    .into_diagnostic()?;
            }
            Ok(())
        }
    }
}

/// The manifest to edit. Editing doesn't need a valid manifest — only the
/// result has to be valid — so a manifest that fails to parse still yields
/// its path, e.g. for `add pixi-build` to fix a `[package]` section that
/// isn't allowed without the flag.
fn manifest_path(located: Result<Workspace, WorkspaceLocatorError>) -> miette::Result<PathBuf> {
    match located {
        Ok(workspace) => Ok(workspace.workspace.provenance.path.clone()),
        Err(WorkspaceLocatorError::Toml(err)) => Ok(PathBuf::from(err.source.name())),
        Err(err) => Err(err.into()),
    }
}
