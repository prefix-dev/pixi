use std::path::PathBuf;

use clap::Parser;
use pixi_config::{Config, ConfigCli};
use pixi_manifest::{
    DependencyOverwriteBehavior, FeatureName, HasFeaturesIter, PrioritizedChannel, SpecType,
};
use pixi_spec::PixiSpec;
use pixi_utils::conda_environment_file::CondaEnvFile;
use rattler_conda_types::Platform;

use miette::IntoDiagnostic;

use super::cli_config::LockFileUpdateConfig;
use crate::{
    WorkspaceLocator,
    cli::cli_config::{PrefixUpdateConfig, WorkspaceConfig},
    environment::sanity_check_workspace,
};

/// Imports an environment into an existing workspace.
#[derive(Parser, Debug, Default)]
#[clap(arg_required_else_help = true, verbatim_doc_comment)]
pub struct Args {
    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    /// File to import into the workspace.
    #[arg(required = true, id = "FILE")]
    pub file: PathBuf,

    /// Which format to interpret the file as.
    #[arg(long)]
    pub format: Option<String>,

    /// The platforms for the imported environment
    #[arg(long = "platform", short, value_name = "PLATFORM")]
    pub platforms: Vec<Platform>,

    /// A name for the created feature
    #[clap(long, short, value_name = "ENVIRONMENT")]
    pub environment: Option<String>,

    /// A name for the created environment
    #[clap(long, short, value_name = "FEATURE")]
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
        if *format != *"conda-env" {
            // TODO: implement conda-lock, conda-txt, pypi-txt
            miette::bail!(
                "Only the conda environment.yml format is supported currently. Please pass `conda-env` to `format`."
            );
        }
        import_conda_env(args).await
    } else {
        import_conda_env(args).await // .or_else(...)
    }
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
    let feature_string = args.feature.clone().unwrap_or(args.environment.clone().unwrap_or(file.name().expect("A name must be provided with `--feature` or `--environment` when an `environment.yml` has no specified `name`.").to_string()));
    let environment_string = args.environment.unwrap_or(args.feature.unwrap_or(file.name().expect("A name must be provided with `--feature` or `--environment` when an `environment.yml` has no specified `name`.").to_string()));
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
        .environment_from_name_or_env_var(Some(environment_string.clone()))
    {
        Err(_) => {
            // add environment if it does not already exist
            workspace.manifest().add_environment(
                environment_string.clone(),
                Some(vec![feature_string.clone()]),
                None,
                true,
            )?;
        }
        Ok(env) => {
            // otherwise, add feature to environment if it is not already there
            if !env.features().any(|f| f.name == feature_name) {
                let (env_name, features, solve_group, no_default_feature) = (
                    env.name().as_str().to_string(),
                    {
                        let mut features: Vec<String> = env
                            .features()
                            .map(|f| f.name.as_str().to_string())
                            .collect();
                        if features.is_empty() {
                            Some(vec![feature_string])
                        } else {
                            Some({
                                features.push(feature_string);
                                features
                            })
                        }
                    },
                    env.solve_group().map(|g| g.name().to_string()),
                    env.no_default_feature(),
                );
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
