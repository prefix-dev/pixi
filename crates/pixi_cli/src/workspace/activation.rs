use std::io::Write;

use clap::Parser;
use fancy_display::FancyDisplay;
use pixi_api::{WorkspaceContext, workspace::ActivationEntry};
use pixi_core::WorkspaceLocator;
use pixi_manifest::{EnvironmentName, FeatureName, HasWorkspaceManifest, TargetSelector};

use crate::{
    cli_config::{WorkspaceConfig, feature_from_flags},
    cli_interface::{CliInterface, cli_context},
};

/// Commands to manage the activation of environments: the scripts that run
/// and the environment variables that are set when an environment is
/// activated.
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
pub enum Command {
    /// Commands to manage the `activation.scripts` list.
    #[clap(visible_alias = "scripts")]
    Script(ScriptArgs),
    /// Commands to manage the `activation.env` table.
    Env(EnvArgs),
    /// List the activation scripts and environment variables in the manifest.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
}

/// The location in the manifest an activation edit applies to.
#[derive(Parser, Debug, Default, Clone)]
pub struct LocationArgs {
    /// The name of the feature to modify.
    #[clap(long, short, value_parser = parse_feature_name)]
    pub feature: Option<FeatureName>,

    /// The environment to modify. The activation is written to the content
    /// defined inline on the environment.
    #[clap(long, short, conflicts_with = "feature")]
    pub environment: Option<EnvironmentName>,

    /// The target selector to modify, e.g. `linux-64`, `unix` or a glob like
    /// `cuda-*`.
    #[clap(long, short)]
    pub target: Option<TargetSelector>,
}

impl LocationArgs {
    fn feature_name(&self) -> FeatureName {
        feature_from_flags(self.environment.as_ref(), self.feature.as_ref())
    }
}

#[derive(Parser, Debug)]
pub struct ScriptArgs {
    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: ScriptCommand,
}

#[derive(Parser, Debug)]
pub enum ScriptCommand {
    /// Add activation script(s) to the end of `activation.scripts`.
    #[clap(visible_alias = "a")]
    Add(ScriptAddRemoveArgs),
    /// Add activation script(s) to the front of `activation.scripts`, moving
    /// them there when they are already present.
    Prepend(ScriptAddRemoveArgs),
    /// Remove activation script(s) from `activation.scripts`.
    #[clap(visible_alias = "rm")]
    Remove(ScriptAddRemoveArgs),
    /// List the activation scripts in the manifest.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
}

#[derive(Parser, Debug)]
pub struct ScriptAddRemoveArgs {
    /// The activation script path(s), relative to the workspace root.
    #[clap(required = true, num_args = 1.., value_parser = parse_script_path)]
    pub script: Vec<String>,

    #[clap(flatten)]
    pub location: LocationArgs,
}

#[derive(Parser, Debug)]
pub struct EnvArgs {
    /// The subcommand to execute
    #[clap(subcommand)]
    pub command: EnvCommand,
}

#[derive(Parser, Debug)]
pub enum EnvCommand {
    /// Set (add or overwrite) activation environment variable(s).
    #[clap(visible_alias = "add")]
    Set(EnvSetArgs),
    /// Remove activation environment variable(s) by name.
    #[clap(visible_aliases = ["rm", "unset"])]
    Remove(EnvRemoveArgs),
    /// List the activation environment variables in the manifest.
    #[clap(visible_alias = "ls")]
    List(ListArgs),
}

#[derive(Parser, Debug)]
pub struct EnvSetArgs {
    /// The environment variable(s) to set, as `KEY=VALUE` pairs.
    #[clap(required = true, num_args = 1.., value_parser = parse_env_assignment, value_name = "KEY=VALUE")]
    pub var: Vec<(String, String)>,

    #[clap(flatten)]
    pub location: LocationArgs,
}

#[derive(Parser, Debug)]
pub struct EnvRemoveArgs {
    /// The name(s) of the environment variable(s) to remove.
    #[clap(required = true, num_args = 1.., value_name = "KEY")]
    pub key: Vec<String>,

    #[clap(flatten)]
    pub location: LocationArgs,
}

#[derive(Parser, Debug, Default)]
pub struct ListArgs {
    /// Only show the activation of this feature.
    #[clap(long, short, value_parser = parse_feature_name)]
    pub feature: Option<FeatureName>,

    /// Only show the activation of features included in this environment.
    #[clap(long, short, conflicts_with = "feature")]
    pub environment: Option<EnvironmentName>,

    /// Only show the activation of this target selector, e.g. `linux-64`,
    /// `unix` or a glob like `cuda-*`.
    #[clap(long, short)]
    pub target: Option<TargetSelector>,
}

/// Parses a `KEY=VALUE` command line argument into its parts. The value may
/// itself contain `=` characters; only the first one separates the key. The
/// key must be a valid environment variable name: activation evaluates
/// `export KEY=...` under the platform's shell, where an invalid identifier
/// either errors on every activation or silently sets a different variable.
fn parse_env_assignment(s: &str) -> Result<(String, String), String> {
    let Some((key, value)) = s.split_once('=') else {
        return Err(format!(
            "expected an assignment in the form KEY=VALUE, got '{s}'"
        ));
    };
    let key = key.trim();
    if key.is_empty() {
        return Err(format!("the variable name is missing in '{s}'"));
    }
    let valid = !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(format!(
            "'{key}' is not a valid environment variable name; use letters, digits and underscores, not starting with a digit"
        ));
    }
    Ok((key.to_string(), value.to_string()))
}

/// Validates an activation script path argument: an empty (or
/// whitespace-only) path can never resolve to a script.
fn parse_script_path(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        return Err("the activation script path may not be empty".to_string());
    }
    Ok(s.to_string())
}

/// Validates a feature name argument: an empty name would silently create a
/// `[feature.""]` table that nothing can reference.
fn parse_feature_name(s: &str) -> Result<FeatureName, String> {
    if s.trim().is_empty() {
        return Err("the feature name may not be empty".to_string());
    }
    Ok(FeatureName::from(s))
}

/// What [`render_list`] should show for each activation table.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ListMode {
    All,
    Scripts,
    Env,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = cli_context(workspace);

    match args.command {
        Command::Script(script_args) => match script_args.command {
            ScriptCommand::Add(args) => {
                workspace_ctx
                    .add_activation_scripts(
                        args.script,
                        false,
                        args.location.target.clone(),
                        args.location.feature_name(),
                    )
                    .await
            }
            ScriptCommand::Prepend(args) => {
                workspace_ctx
                    .add_activation_scripts(
                        args.script,
                        true,
                        args.location.target.clone(),
                        args.location.feature_name(),
                    )
                    .await
            }
            ScriptCommand::Remove(args) => {
                workspace_ctx
                    .remove_activation_scripts(
                        args.script,
                        args.location.target.clone(),
                        args.location.feature_name(),
                    )
                    .await
            }
            ScriptCommand::List(args) => list(&workspace_ctx, args, ListMode::Scripts).await,
        },
        Command::Env(env_args) => match env_args.command {
            EnvCommand::Set(args) => {
                workspace_ctx
                    .set_activation_env(
                        args.var,
                        args.location.target.clone(),
                        args.location.feature_name(),
                    )
                    .await
            }
            EnvCommand::Remove(args) => {
                for key in &args.key {
                    if key.contains('=') {
                        miette::bail!(
                            "pass just the variable name to remove, not an assignment: '{key}'"
                        );
                    }
                }
                workspace_ctx
                    .remove_activation_env(
                        args.key,
                        args.location.target.clone(),
                        args.location.feature_name(),
                    )
                    .await
            }
            EnvCommand::List(args) => list(&workspace_ctx, args, ListMode::Env).await,
        },
        Command::List(args) => list(&workspace_ctx, args, ListMode::All).await,
    }
}

async fn list(
    workspace_ctx: &WorkspaceContext<CliInterface>,
    args: ListArgs,
    mode: ListMode,
) -> miette::Result<()> {
    // Resolve the `--environment` filter to the set of features the
    // environment consists of.
    let feature_filter: Option<Vec<FeatureName>> = match (&args.feature, &args.environment) {
        (Some(feature), _) => {
            let manifest = workspace_ctx.workspace().workspace_manifest();
            if manifest.all_features().all(|(name, _)| name != feature) {
                return Err(miette::miette!(
                    "the feature '{}' does not exist",
                    feature.as_str()
                ));
            }
            Some(vec![feature.clone()])
        }
        (None, Some(environment)) => {
            let manifest = workspace_ctx.workspace().workspace_manifest();
            let environment = manifest
                .environments
                .find(environment)
                .ok_or_else(|| miette::miette!("the environment '{environment}' does not exist"))?;
            let mut features = environment.features.clone();
            if !environment.no_default_feature {
                features.push(FeatureName::Default);
            }
            Some(features)
        }
        (None, None) => None,
    };

    let entries: Vec<ActivationEntry> = workspace_ctx
        .list_activation()
        .await
        .into_iter()
        .filter(|entry| {
            feature_filter
                .as_ref()
                .is_none_or(|features| features.contains(&entry.feature))
        })
        .filter(|entry| {
            args.target
                .as_ref()
                .is_none_or(|target| entry.target.as_ref() == Some(target))
        })
        .filter(|entry| match mode {
            ListMode::All => true,
            ListMode::Scripts => !entry.scripts.is_empty(),
            ListMode::Env => !entry.env.is_empty(),
        })
        .collect();

    let mut output = String::new();
    for entry in &entries {
        let name = match entry.feature.environment_name() {
            Some(environment) => format!(
                "{} {}",
                console::style("Environment:").bold().bright(),
                environment.fancy_display()
            ),
            None => format!(
                "{} {}",
                console::style("Feature:").bold().bright(),
                entry.feature.fancy_display()
            ),
        };
        let target = entry
            .target
            .as_ref()
            .map(|target| format!(" {} {}", console::style("Target:").bold().bright(), target))
            .unwrap_or_default();
        output.push_str(&format!("{name}{target}\n"));

        if mode != ListMode::Env && !entry.scripts.is_empty() {
            output.push_str("  scripts:\n");
            for script in &entry.scripts {
                output.push_str(&format!("    - {}\n", console::style(script).green()));
            }
        }
        if mode != ListMode::Scripts && !entry.env.is_empty() {
            output.push_str("  env:\n");
            for (key, value) in &entry.env {
                output.push_str(&format!(
                    "    {} = \"{value}\"\n",
                    console::style(key).cyan()
                ));
            }
        }
    }

    if entries.is_empty() {
        let what = match mode {
            ListMode::All => "activation scripts or environment variables",
            ListMode::Scripts => "activation scripts",
            ListMode::Env => "activation environment variables",
        };
        let scope = if args.feature.is_some() || args.environment.is_some() || args.target.is_some()
        {
            "for the given filter"
        } else {
            "in the manifest"
        };
        output.push_str(&format!("No {what} found {scope}.\n"));
    }

    let _ = write!(std::io::stdout(), "{output}").inspect_err(|e| {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
    });

    Ok(())
}
