use std::path::PathBuf;
use std::str::FromStr;

use clap::{Parser, ValueEnum};
use pixi_config::{Config, ConfigCli};
use pixi_manifest::{
    DependencyOverwriteBehavior, EnvironmentName, FeatureName, HasFeaturesIter, PrioritizedChannel,
    SpecType,
};
use pixi_spec::PixiSpec;
use pixi_utils::conda_environment_file::CondaEnvFile;
use rattler_conda_types::Platform;

use miette::{Diagnostic, IntoDiagnostic, Result};
use thiserror::Error;

use super::cli_config::LockFileUpdateConfig;
use crate::{
    WorkspaceLocator,
    cli::cli_config::{PrefixUpdateConfig, WorkspaceConfig},
    environment::sanity_check_workspace,
};

#[derive(Parser, Debug, Clone, PartialEq, ValueEnum)]
pub enum ImportFileFormat {
    // TODO: implement conda-lock, conda-txt, pypi-txt
    CondaEnv,
}

/// Imports a file into an environment in an existing workspace.
///
/// If `--format` isn't provided, `import` will try to guess the format based on the file extension.
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// File to import into the workspace.
    #[arg(id = "FILE")]
    pub file: PathBuf,

    /// Which format to interpret the file as.
    #[arg(long, ignore_case = true)]
    pub format: Option<ImportFileFormat>,

    /// The platforms for the imported environment
    #[arg(long = "platform", short, value_name = "PLATFORM")]
    pub platforms: Vec<Platform>,

    /// A name for the created environment
    #[clap(long, short)]
    pub environment: Option<String>,

    /// A name for the created feature
    #[clap(long, short)]
    pub feature: Option<String>,

    #[clap(flatten)]
    pub prefix_update_config: PrefixUpdateConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub config: ConfigCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    if let Some(format) = &args.format {
        if *format != ImportFileFormat::CondaEnv {
            miette::bail!(
                "Only the conda environment.yml format is supported currently. Please pass `conda-env` to `format`."
            );
        }
        import_conda_env(args).await
    } else {
        import_conda_env(args).await // .or_else(...)
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error("Missing name: provide --feature or --environment, or set `name:` in environment.yml")]
struct MissingEnvironmentName;

fn get_feature_and_environment(
    feature_arg: &Option<String>,
    environment_arg: &Option<String>,
    file: &CondaEnvFile,
) -> Result<(String, String), miette::Report> {
    let fallback = || {
        file.name()
            .map(|s| s.to_string())
            .ok_or(MissingEnvironmentName)
    };

    let feature_string = match (feature_arg, environment_arg) {
        (Some(f), _) => f.clone(),
        (_, Some(e)) => e.clone(),
        _ => fallback()?,
    };

    let environment_string = match (environment_arg, feature_arg) {
        (Some(e), _) => e.clone(),
        (_, Some(f)) => f.clone(),
        _ => fallback()?,
    };

    Ok((feature_string, environment_string))
}

async fn import_conda_env(args: Args) -> miette::Result<()> {
    let (file, platforms, workspace_config) = (args.file, args.platforms, args.workspace_config);
    let config = Config::from(args.config);

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(config.clone());

    sanity_check_workspace(&workspace).await?;

    let mut workspace = workspace.modify()?;
    let channel_config = workspace.workspace().channel_config();

    // TODO: add dry_run logic to import

    let file = CondaEnvFile::from_path(&file)?;
    let (feature_string, environment_string) =
        get_feature_and_environment(&args.feature, &args.environment, &file)?;
    let feature_name = FeatureName::from(feature_string.clone());

    // Add the platforms if they are not already present
    if !platforms.is_empty() {
        workspace
            .manifest()
            .add_platforms(platforms.iter(), &feature_name)?;
    }

    // TODO: handle `variables` section
    // let env_vars = file.variables();

    // TODO: Improve this:
    //  - Use .condarc as channel config
    let (conda_deps, pypi_deps, channels) = file.to_manifest(&config.clone())?;
    workspace.manifest().add_channels(
        channels.iter().map(|c| PrioritizedChannel::from(c.clone())),
        &feature_name,
        false,
    )?;

    for spec in conda_deps {
        // Determine the name of the package to add
        let (Some(package_name), spec) = spec.clone().into_nameless() else {
            miette::bail!(
                "{} does not support wildcard dependencies",
                pixi_utils::executable_name()
            );
        };
        let spec = PixiSpec::from_nameless_matchspec(spec, &channel_config);
        workspace.manifest().add_dependency(
            &package_name,
            &spec,
            SpecType::Run,
            &platforms,
            &feature_name,
            DependencyOverwriteBehavior::Overwrite,
        )?;
    }
    for requirement in pypi_deps {
        workspace.manifest().add_pep508_dependency(
            (&requirement, None),
            &platforms,
            &feature_name,
            None,
            DependencyOverwriteBehavior::Overwrite,
            None,
        )?;
    }

    match workspace
        .workspace()
        .environment(&EnvironmentName::from_str(&environment_string)?)
    {
        None => {
            // add environment if it does not already exist
            workspace.manifest().add_environment(
                environment_string.clone(),
                Some(vec![feature_string.clone()]),
                None,
                true,
            )?;
        }
        Some(env) => {
            // otherwise, add feature to environment if it is not already there
            if !env.features().any(|f| f.name == feature_name) {
                let env_name = env.name().as_str().to_string();
                let features = {
                    let features = env
                        .features()
                        .map(|f| f.name.as_str().to_string())
                        .chain(std::iter::once(feature_string))
                        .collect();
                    Some(features)
                };
                let solve_group = env.solve_group().map(|g| g.name().to_string());
                let no_default_feature = env.no_default_feature();

                workspace.manifest().add_environment(
                    env_name,
                    features,
                    solve_group,
                    no_default_feature,
                )?;
            }
        }
    }

    let workspace = workspace.save().await.into_diagnostic()?;

    eprintln!(
        "{}Imported to {}",
        console::style(console::Emoji("✔ ", "")).green(),
        // Canonicalize the path to make it more readable, but if it fails just use the path as
        // is.
        workspace.workspace.provenance.path.display()
    );

    Ok(())
}
