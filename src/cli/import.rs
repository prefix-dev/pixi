use std::path::PathBuf;
use std::str::FromStr;

use clap::{Parser, ValueEnum};
use pixi_config::{Config, ConfigCli};
use pixi_manifest::{EnvironmentName, FeatureName, HasFeaturesIter, PrioritizedChannel};
use pixi_utils::conda_environment_file::CondaEnvFile;
use pixi_uv_conversions::convert_uv_requirements_to_pep508;
use rattler_conda_types::Platform;

use uv_client::BaseClientBuilder;
use uv_requirements_txt::RequirementsTxt;

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
    // TODO: implement conda-lock, conda-txt
    CondaEnv,
    PypiTxt,
}

/// Imports a file into an environment in an existing workspace.
///
/// If `--format` isn't provided, `import` will try to guess the format based on the file extension.
#[derive(Parser, Debug, Default, Clone)]
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
        import(args.clone(), format).await
    } else if let Ok(result) = import(args.clone(), &ImportFileFormat::CondaEnv).await {
        return Ok(result);
    } else if let Ok(result) = import(args, &ImportFileFormat::PypiTxt).await {
        return Ok(result);
    } else {
        miette::bail!(
            "Tried all formats for input file, but none were successful. Pass a `format` argument to see the specific error for that format."
        )
    }
}

#[derive(Debug, Error, Diagnostic)]
#[error(
    "Missing name: provide --feature or --environment, or set `name:` in input file for the conda-env format."
)]
struct MissingEnvironmentName;

fn get_feature_and_environment(
    feature_arg: &Option<String>,
    environment_arg: &Option<String>,
    fallback: impl Fn() -> Result<String, MissingEnvironmentName>,
) -> Result<(String, String), miette::Report> {
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

fn convert_uv_requirements_txt_to_pep508(
    reqs_txt: uv_requirements_txt::RequirementsTxt,
) -> Result<Vec<pep508_rs::Requirement>, miette::Error> {
    let mut uv_requirements: Vec<uv_pep508::Requirement<uv_pypi_types::VerbatimParsedUrl>> =
        reqs_txt
            .requirements
            .into_iter()
            .map(|r| match r.requirement {
                uv_requirements_txt::RequirementsTxtRequirement::Named(req) => Ok(req),
                uv_requirements_txt::RequirementsTxtRequirement::Unnamed(_) => {
                    Err(miette::miette!(
                        "Error parsing input file: unnamed requirements are currently unsupported."
                    ))
                }
            })
            .collect::<Result<_, _>>()?;
    uv_requirements.extend(reqs_txt.constraints);

    let requirements =
        convert_uv_requirements_to_pep508(uv_requirements.iter()).into_diagnostic()?;

    Ok(requirements)
}

async fn import(args: Args, format: &ImportFileFormat) -> miette::Result<()> {
    let (input_file, platforms, workspace_config) =
        (args.file, args.platforms, args.workspace_config);
    let config = Config::from(args.config);

    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(config.clone());

    sanity_check_workspace(&workspace).await?;

    let mut workspace = workspace.modify()?;

    // TODO: add dry_run logic to import

    let (env_file, feature_string, environment_string) = match format {
        ImportFileFormat::CondaEnv => {
            let env_file = CondaEnvFile::from_path(&input_file)?;
            let fallback = || {
                env_file
                    .name()
                    .map(|s| s.to_string())
                    .ok_or(MissingEnvironmentName)
            };
            let (feature_string, environment_string) =
                get_feature_and_environment(&args.feature, &args.environment, fallback)?;
            (Some(env_file), feature_string, environment_string)
        }
        ImportFileFormat::PypiTxt => {
            let (feature_string, environment_string) =
                get_feature_and_environment(&args.feature, &args.environment, || {
                    Err(MissingEnvironmentName)
                })?;
            (None, feature_string, environment_string)
        }
    };

    let feature_name = FeatureName::from(feature_string.clone());

    // Add the platforms if they are not already present
    if !platforms.is_empty() {
        workspace
            .manifest()
            .add_platforms(platforms.iter(), &feature_name)?;
    }

    let (conda_deps, pypi_deps) = match format {
        ImportFileFormat::CondaEnv => {
            // TODO: handle `variables` section
            // let env_vars = file.variables();

            // TODO: Improve this:
            //  - Use .condarc as channel config
            let env_file = env_file.expect("Some is returned for CondaEnv");
            let (conda_deps, pypi_deps, channels) = env_file.to_manifest(&config.clone())?;
            workspace.manifest().add_channels(
                channels.iter().map(|c| PrioritizedChannel::from(c.clone())),
                &feature_name,
                false,
            )?;

            (conda_deps, pypi_deps)
        }
        ImportFileFormat::PypiTxt => {
            let reqs_txt = RequirementsTxt::parse(
                &input_file,
                workspace.workspace().root(),
                &BaseClientBuilder::new(),
            )
            .await
            .into_diagnostic()?;
            let pypi_deps = convert_uv_requirements_txt_to_pep508(reqs_txt)?;

            (vec![], pypi_deps)
        }
    };

    workspace.add_specs(conda_deps, pypi_deps, &platforms, &feature_name)?;

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
