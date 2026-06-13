use std::path::PathBuf;
use std::str::FromStr;

use clap::{Parser, ValueEnum};
use indexmap::IndexSet;
use pixi_api::workspace::platforms::resolve_platforms;
use pixi_config::ConfigCli;
use pixi_core::{WorkspaceLocator, environment::sanity_check_workspace};
use pixi_manifest::{
    EnvironmentName, FeatureName, HasFeaturesIter, PixiPlatformName, PrioritizedChannel,
};
use pixi_utils::conda_environment_file::CondaEnvFile;
use pixi_uv_conversions::convert_uv_requirements_to_pep508;

use tracing::warn;
use uv_requirements_txt::RequirementsTxt;

use miette::{Diagnostic, IntoDiagnostic, Result};
use thiserror::Error;

use crate::cli_config::WorkspaceConfig;

#[derive(Parser, Debug, Clone, PartialEq, ValueEnum)]
pub enum ImportFileFormat {
    // TODO: implement conda-lock, conda-txt
    CondaEnv,
    PypiTxt,
}

/// Imports a file into an environment in an existing workspace.
///
/// If `--format` isn't provided, `import` will try each format in turn
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

    /// The platforms for the imported environment. Accepts a workspace
    /// platform name; a bare conda subdir (e.g. `linux-64`) is also
    /// accepted. Names that aren't yet declared get auto-added as subdir
    /// platforms.
    #[arg(long = "platform", short, value_name = "PLATFORM")]
    pub platforms: Vec<PixiPlatformName>,

    /// A name for the created environment
    #[clap(long, short)]
    pub environment: Option<String>,

    /// A name for the created feature
    #[clap(long, short)]
    pub feature: Option<String>,

    #[clap(flatten)]
    pub config: ConfigCli,

    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    if let Some(format) = &args.format {
        import(args.clone(), format).await
    } else if let Ok(result) = import(args.clone(), &ImportFileFormat::CondaEnv).await {
        Ok(result)
    } else if let Ok(result) = import(args, &ImportFileFormat::PypiTxt).await {
        Ok(result)
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
) -> Result<(FeatureName, EnvironmentName), miette::Report> {
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

    Ok((
        FeatureName::from(feature_string),
        EnvironmentName::from_str(&environment_string)?,
    ))
}

fn convert_uv_requirements_txt_to_pep508(
    reqs_txt: uv_requirements_txt::RequirementsTxt,
) -> Result<Vec<pep508_rs::Requirement>, miette::Error> {
    let uv_requirements: Vec<uv_pep508::Requirement<uv_pypi_types::VerbatimParsedUrl>> = reqs_txt
        .requirements
        .into_iter()
        .map(|r| match r.requirement {
            uv_requirements_txt::RequirementsTxtRequirement::Named(req) => Ok(req),
            uv_requirements_txt::RequirementsTxtRequirement::Unnamed(_) => Err(miette::miette!(
                "Error parsing input file: unnamed requirements are currently unsupported."
            )),
        })
        .collect::<Result<_, _>>()?;
    if !reqs_txt.constraints.is_empty() {
        warn!(
            "Constraints detected in input file, but these are currently unsupported. Continuing without applying constraints..."
        )
    }

    let requirements =
        convert_uv_requirements_to_pep508(uv_requirements.iter()).into_diagnostic()?;

    Ok(requirements)
}

async fn import(args: Args, format: &ImportFileFormat) -> miette::Result<()> {
    let source = args.config_source.source();
    let (input_file, platforms, workspace_config) =
        (args.file, args.platforms, args.workspace_config);

    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(source)
        .with_search_start(workspace_config.workspace_locator_start())
        .locate()?
        .with_cli_config(args.config);

    sanity_check_workspace(&workspace).await?;

    let mut workspace = workspace.modify()?;

    // TODO: add dry_run logic to import

    enum ProcessedInput {
        CondaEnv(CondaEnvFile),
        PypiTxt,
    }

    let (processed_input, feature_name, environment_name) = match format {
        ImportFileFormat::CondaEnv => {
            let env_file = CondaEnvFile::from_path(&input_file)?;
            let fallback = || {
                env_file
                    .name()
                    .map(|s| s.to_string())
                    .ok_or(MissingEnvironmentName)
            };
            let (feature_name, environment_name) =
                get_feature_and_environment(&args.feature, &args.environment, fallback)?;

            (
                ProcessedInput::CondaEnv(env_file),
                feature_name,
                environment_name,
            )
        }
        ImportFileFormat::PypiTxt => {
            let (feature_name, environment_name) =
                get_feature_and_environment(&args.feature, &args.environment, || {
                    Err(MissingEnvironmentName)
                })?;
            (ProcessedInput::PypiTxt, feature_name, environment_name)
        }
    };

    // Resolve the platform names. Import doesn't have a target workspace
    // yet (or at least, doesn't read its platforms here), so each name has
    // to parse as a conda subdir. The user can rename the resulting
    // entries afterwards via `workspace platform edit`.
    let pixi_platforms = resolve_platforms(&IndexSet::default(), &platforms)?;
    let platform_names: Vec<pixi_manifest::PixiPlatformName> =
        pixi_platforms.iter().map(|p| p.name().clone()).collect();
    if !pixi_platforms.is_empty() {
        workspace
            .manifest()
            .add_platforms(pixi_platforms.iter(), &feature_name)?;
    }

    let (conda_deps, pypi_deps) = match processed_input {
        ProcessedInput::CondaEnv(env_file) => {
            // TODO: handle `variables` section
            // let env_vars = file.variables();

            // TODO: Improve this:
            //  - Use .condarc as channel config
            let (conda_deps, pypi_deps, channels) =
                env_file.to_manifest(workspace.workspace().config())?;
            workspace.manifest().add_channels(
                channels.iter().map(|c| PrioritizedChannel::from(c.clone())),
                &feature_name,
                false,
            )?;

            (conda_deps, pypi_deps)
        }
        ProcessedInput::PypiTxt => {
            let reqs_txt = RequirementsTxt::parse(&input_file, workspace.workspace().root())
                .await
                .into_diagnostic()?;
            let pypi_deps = convert_uv_requirements_txt_to_pep508(reqs_txt)?;

            (vec![], pypi_deps)
        }
    };

    let targets = workspace.target_selectors_for_platforms(&platform_names);
    workspace.add_specs(conda_deps, pypi_deps, &targets, &feature_name)?;

    match workspace.workspace().environment(&environment_name) {
        None => {
            // add environment if it does not already exist
            workspace.manifest().add_environment(
                environment_name.to_string(),
                Some(vec![feature_name.to_string()]),
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
                        .chain(std::iter::once(feature_name.to_string()))
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
