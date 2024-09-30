use std::str::FromStr;

use clap::Parser;
use indexmap::IndexMap;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, PackageName, Platform};

use crate::{
    cli::{global::revert_after_error, has_specs::HasSpecs},
    global::{self, EnvDir, EnvironmentName, ExposedName, Mapping, Project},
    prefix::Prefix,
};
use pixi_config::{self, Config, ConfigCli};

/// Installs the defined package in a globally accessible location.
#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct Args {
    /// Specifies the packages that are to be installed.
    #[arg(num_args = 1..)]
    packages: Vec<String>,

    /// The channels to consider as a name or a url.
    /// Multiple channels can be specified by using this field multiple times.
    ///
    /// When specifying a channel, it is common that the selected channel also
    /// depends on the `conda-forge` channel.
    ///
    /// By default, if no channel is provided, `conda-forge` is used.
    #[clap(long = "channel", short = 'c', value_name = "CHANNEL")]
    channels: Vec<NamedChannelOrUrl>,

    #[clap(short, long)]
    platform: Option<Platform>,

    /// Ensures that all packages will be installed in the same environment
    #[clap(short, long)]
    environment: Option<EnvironmentName>,

    /// Add one or more `MAPPING` for environment `ENV` which describe which executables are exposed.
    /// The syntax for `MAPPING` is `exposed_name=executable_name`, so for example `python3.10=python`.
    #[arg(long)]
    expose: Vec<Mapping>,

    /// Answer yes to all questions.
    #[clap(short = 'y', long = "yes", long = "assume-yes")]
    assume_yes: bool,

    #[clap(flatten)]
    config: ConfigCli,
}

impl HasSpecs for Args {
    fn packages(&self) -> Vec<&str> {
        self.packages.iter().map(AsRef::as_ref).collect()
    }
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let project_original = global::Project::discover_or_create(args.assume_yes)
        .await?
        .with_cli_config(config.clone());

    async fn apply_changes(args: Args, project: &mut Project) -> Result<(), miette::Error> {
        let specs = args.specs()?;

        let env_names = match &args.environment {
            Some(env_name) => Vec::from([env_name.clone()]),
            None => specs
                .iter()
                .map(|(package_name, _)| package_name.as_normalized().parse().into_diagnostic())
                .collect::<miette::Result<Vec<_>>>()?,
        };

        if !args.expose.is_empty() && env_names.len() != 1 {
            miette::bail!("Cannot add exposed mappings for more than one environment");
        }

        let multiple_envs = env_names.len() > 1;

        for env_name in env_names {
            let specs = if multiple_envs {
                specs
                    .clone()
                    .into_iter()
                    .filter(|(package_name, _)| env_name.as_str() == package_name.as_source())
                    .collect()
            } else {
                specs.clone()
            };

            setup_environment(&env_name, project, args.clone(), specs).await?;
        }
        project.manifest.save().await?;
        Ok(())
    }

    let mut project = project_original.clone();
    if let Err(err) = apply_changes(args, &mut project).await {
        revert_after_error(&project_original)
            .await
            .wrap_err("Could not install packages. Reverting also failed.")?;
        return Err(err);
    }
    Ok(())
}

async fn setup_environment(
    env_name: &EnvironmentName,
    project: &mut Project,
    args: Args,
    specs: IndexMap<PackageName, MatchSpec>,
) -> miette::Result<()> {
    // Modify the project to include the new environment
    if project.manifest.parsed.envs.contains_key(env_name) {
        project.manifest.remove_environment(env_name)?;
    }

    let channels = if args.channels.is_empty() {
        project.config().default_channels()
    } else {
        args.channels.clone()
    };
    project.manifest.add_environment(env_name, Some(channels))?;

    if let Some(platform) = args.platform {
        project.manifest.set_platform(env_name, platform)?;
    }

    // Add the dependencies to the environment
    for (package_name, spec) in &specs {
        project
            .manifest
            .add_dependency(env_name, package_name, spec)?;
    }

    // Installing the environment to be able to find the bin paths later
    project.install_environment(env_name).await?;

    if args.expose.is_empty() {
        // Add the expose binaries for all the packages that were requested to the manifest
        for (package_name, _spec) in &specs {
            let env_dir = EnvDir::from_env_root(project.env_root.clone(), env_name.clone()).await?;
            let prefix = Prefix::new(env_dir.path());
            let prefix_package = prefix.find_designated_package(package_name).await?;
            for (executable_name, _) in prefix.find_executables(&[prefix_package]) {
                let mapping =
                    Mapping::new(ExposedName::from_str(&executable_name)?, executable_name);
                project.manifest.add_exposed_mapping(env_name, &mapping)?;
            }
        }
    } else {
        // Only add the exposed mappings that were requested
        for mapping in &args.expose {
            project.manifest.add_exposed_mapping(env_name, mapping)?;
        }
    }

    // Expose executables of the new environment
    project
        .expose_executables_from_environment(env_name)
        .await?;
    Ok(())
}
