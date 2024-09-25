use clap::Parser;
use miette::Context;
use rattler_conda_types::{NamedChannelOrUrl, Platform};

use crate::{
    cli::{global::revert_after_error, has_specs::HasSpecs},
    global::{self, EnvironmentName},
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

        for (package_name, spec) in args.specs()? {
            let env_name = match &args.environment {
                Some(env_name) => env_name,
                None => &package_name.as_normalized().parse()?,
            };

            if project_modified.manifest.parsed.envs.contains_key(env_name) {
                for channel in &args.channels {
                    project_modified.manifest.add_channel(env_name, channel)?;
                }
            } else {
                project_modified
                    .manifest
                    .add_environment(env_name, Some(args.channels.clone()))?;
            }

            if let Some(platform) = args.platform {
                project_modified.manifest.set_platform(env_name, platform)?;
            }

            project_modified
                .manifest
                .add_dependency(env_name, &package_name, &spec)?;
        }
        project_modified.manifest.save().await?;
        global::sync(&project_modified, config).await?;
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
