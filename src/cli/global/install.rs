use std::str::FromStr;

use clap::Parser;
use miette::{Context, IntoDiagnostic};
use rattler_conda_types::{NamedChannelOrUrl, Platform};

use crate::{
    cli::{global::revert_after_error, has_specs::HasSpecs},
    global::{self, EnvDir, EnvironmentName, ExposedName, Mapping},
    prefix::Prefix,
};
use pixi_config::{self, Config, ConfigCli};

/// Installs the defined package in a global accessible location.
#[derive(Parser, Debug)]
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
    expose: Vec<global::Mapping>,

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

    async fn apply_changes(
        args: Args,
        project_original: global::Project,
        config: &Config,
    ) -> Result<(), miette::Error> {
        let mut project_modified = project_original;
        let specs = args.specs()?;

        let env_names = match &args.environment {
            Some(env_name) => Vec::from([env_name.clone()]),
            None => specs
                .iter()
                .map(|(package_name, _)| package_name.as_normalized().parse().into_diagnostic())
                .collect::<miette::Result<Vec<_>>>()?,
        };

        if !args.expose.is_empty() {
            if env_names.len() == 1 {
                for mapping in &args.expose {
                    project_modified
                        .manifest
                        .add_exposed_mapping(&env_names[0], mapping)?;
                }
            } else {
                miette::bail!("Cannot add exposed mappings for more than one environment");
            }
        }

        for env_name in &env_names {
            if project_modified.manifest.parsed.envs.contains_key(env_name) {
                for channel in &args.channels {
                    project_modified.manifest.add_channel(env_name, channel)?;
                }
            } else {
                let channels = if args.channels.is_empty() {
                    config.default_channels()
                } else {
                    args.channels.clone()
                };
                project_modified
                    .manifest
                    .add_environment(env_name, Some(channels))?;
            }

            if let Some(platform) = args.platform {
                project_modified.manifest.set_platform(env_name, platform)?;
            }
        }

        for ((package_name, spec), env_name) in specs.iter().zip(env_names.iter().cycle()) {
            project_modified
                .manifest
                .add_dependency(env_name, package_name, spec)?;
        }
        project_modified.manifest.save().await?;
        global::sync(&project_modified, config).await?;

        if args.expose.is_empty() {
            for ((package_name, _), env_name) in specs.iter().zip(env_names.iter().cycle()) {
                let env_dir =
                    EnvDir::from_env_root(project_modified.env_root.clone(), env_name.clone())
                        .await?;
                let prefix = Prefix::new(env_dir.path());
                let prefix_package = prefix.find_designated_package(package_name).await?;
                for (executable_name, _) in prefix.find_executables(&[prefix_package]) {
                    let mapping =
                        Mapping::new(ExposedName::from_str(&executable_name)?, executable_name);
                    project_modified
                        .manifest
                        .add_exposed_mapping(env_name, &mapping)?;
                }
            }
            project_modified.manifest.save().await?;
            global::sync(&project_modified, config).await?;
        }
        Ok(())
    }

    if let Err(err) = apply_changes(args, project_original.clone(), &config).await {
        revert_after_error(&project_original, &config)
            .await
            .wrap_err("Could not install packages. Reverting also failed.")?;
        return Err(err);
    }
    Ok(())
}
