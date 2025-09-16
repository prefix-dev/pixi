use std::{collections::BTreeSet, collections::HashMap, path::Path, str::FromStr, sync::LazyLock};

use clap::{Parser, ValueHint};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_config::{self, Config, ConfigCli};
use pixi_core::environment::list::{PackageToOutput, print_package_table};
use pixi_progress::{await_in_progress, global_multi_progress, wrap_in_progress};
use pixi_utils::prefix::Prefix;
use pixi_utils::{AsyncPrefixGuard, EnvironmentHash, reqwest::build_reqwest_clients};
use rattler::{
    install::{IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{GenericVirtualPackage, MatchSpec, PackageName, Platform};
use rattler_solve::{SolverImpl, SolverTask, resolvo::Solver};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use reqwest_middleware::ClientWithMiddleware;
use uv_configuration::RAYON_INITIALIZE;

use crate::cli_config::ChannelsConfig;

/// Run a command and install it in a temporary environment.
///
/// Remove the temporary environments with `pixi clean cache --exec`.
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// The executable to run, followed by any arguments.
    #[clap(num_args = 1.., value_hint = ValueHint::CommandWithArguments)]
    pub command: Vec<String>,

    /// Matchspecs of package to install.
    /// If this is not provided, the package is guessed from the command.
    #[clap(long = "spec", short = 's', value_name = "SPEC")]
    pub specs: Vec<MatchSpec>,

    /// Matchspecs of package to install, while also guessing a package
    /// from the command.
    #[clap(long, short = 'w', conflicts_with = "specs")]
    pub with: Vec<MatchSpec>,

    #[clap(flatten)]
    channels: ChannelsConfig,

    /// The platform to create the environment for.
    #[clap(long, short, default_value_t = Platform::current())]
    pub platform: Platform,

    /// If specified a new environment is always created even if one already
    /// exists.
    #[clap(long)]
    pub force_reinstall: bool,

    /// Before executing the command, list packages in the environment
    /// Specify `--list=some_regex` to filter the shown packages
    #[clap(long = "list", num_args = 0..=1, default_missing_value = "", require_equals = true)]
    pub list: Option<String>,

    /// Disable modification of the PS1 prompt to indicate the temporary environment
    #[clap(long)]
    pub no_modify_ps1: bool,

    #[clap(flatten)]
    pub config: ConfigCli,
}

/// CLI entry point for `pixi exec`
pub async fn execute(args: Args) -> miette::Result<()> {
    let config = Config::with_cli_config(&args.config);
    let cache_dir = pixi_config::get_cache_dir().context("failed to determine cache directory")?;

    let mut command_iter = args.command.iter();
    let command = command_iter.next().ok_or_else(|| miette::miette!(help ="i.e when specifying specs explicitly use a command at the end: `pixi exec -s python==3.12 python`", "missing required command to execute",))?;
    let (_, client) = build_reqwest_clients(Some(&config), None)?;

    // Determine the specs for installation and for the environment name.
    let mut name_specs = args.specs.clone();
    name_specs.extend(args.with.clone());

    let mut install_specs = name_specs.clone();

    // Guess a package from the command if no specs were provided at all OR if --with is used
    let should_guess_package = name_specs.is_empty() || !args.with.is_empty();
    if should_guess_package {
        install_specs.push(guess_package_spec(command));
    }

    // Create the environment to run the command in.
    let prefix = create_exec_prefix(
        &args,
        &install_specs,
        &cache_dir,
        &config,
        &client,
        should_guess_package,
    )
    .await?;

    // Get environment variables from the activation
    let mut activation_env = run_activation(&prefix).await?;

    // Collect unique package names for environment naming
    let package_names: BTreeSet<String> = name_specs
        .iter()
        .filter_map(|spec| spec.name.as_ref().map(|n| n.as_normalized().to_string()))
        .collect();

    if !package_names.is_empty() {
        let env_name = format!("temp:{}", package_names.into_iter().format(","));

        activation_env.insert("PIXI_ENVIRONMENT_NAME".into(), env_name.clone());

        if !args.no_modify_ps1 && std::env::current_dir().is_ok() {
            let (prompt_var, prompt_value) = if cfg!(windows) {
                ("_PIXI_PROMPT", format!("(pixi:{}) $P$G", env_name))
            } else {
                ("PS1", format!(r"(pixi:{}) [\w] \$", env_name))
            };

            activation_env.insert(prompt_var.into(), prompt_value);

            if cfg!(windows) {
                activation_env.insert("PROMPT".into(), String::from("$P$G"));
            }
        }
    }

    // Ignore CTRL+C so that the child is responsible for its own signal handling.
    let _ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

    // Spawn the command
    let mut cmd = std::process::Command::new(command);
    let command_args_vec: Vec<_> = command_iter.collect();
    cmd.args(&command_args_vec);

    // On Windows, when using cmd.exe or cmd, we need to pass the full environment
    // because cmd.exe requires access to all environment variables (including prompt variables)
    // to properly display the modified prompt
    if cfg!(windows) && (command.to_lowercase().ends_with("cmd.exe") || command == "cmd") {
        let mut env = std::env::vars().collect::<HashMap<String, String>>();
        env.extend(activation_env);
        cmd.envs(env);
    } else {
        cmd.envs(activation_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    let status = cmd
        .status()
        .into_diagnostic()
        .with_context(|| format!("failed to execute '{}'", &command))?;

    // Return the exit code of the command
    std::process::exit(status.code().unwrap_or(1));
}

/// Creates a prefix for the `pixi exec` command.
pub async fn create_exec_prefix(
    args: &Args,
    specs: &[MatchSpec],
    cache_dir: &Path,
    config: &Config,
    client: &ClientWithMiddleware,
    has_guessed_package: bool,
) -> miette::Result<Prefix> {
    let command = args.command.first().expect("missing required command");
    let specs = specs.to_vec();

    let channels = args
        .channels
        .resolve_from_config(config)?
        .iter()
        .map(|c| c.base_url.to_string())
        .collect();

    let environment_hash =
        EnvironmentHash::new(command.clone(), specs.clone(), channels, args.platform);

    let prefix = Prefix::new(
        cache_dir
            .join(pixi_consts::consts::CACHED_ENVS_DIR)
            .join(environment_hash.name()),
    );

    let guard = AsyncPrefixGuard::new(prefix.root())
        .await
        .into_diagnostic()
        .context("failed to create prefix guard")?;

    let mut write_guard = await_in_progress("acquiring write lock on prefix", |_| guard.write())
        .await
        .into_diagnostic()
        .context("failed to acquire write lock to prefix guard")?;

    // If the environment already exists, and we are not forcing a
    // reinstallation, we can return early.
    if write_guard.is_ready() && !args.force_reinstall {
        tracing::info!(
            "reusing existing environment in {}",
            prefix.root().display()
        );
        write_guard.finish().await.into_diagnostic()?;
        return Ok(prefix);
    }

    // Update the prefix to indicate that we are installing it.
    write_guard
        .begin()
        .await
        .into_diagnostic()
        .context("failed to write lock status to prefix guard")?;

    // Construct a gateway to get repodata.
    let gateway = config.gateway().with_client(client.clone()).finish();

    let channels = args.channels.resolve_from_config(config)?;

    // Get the repodata for the specs
    let repodata = await_in_progress("fetching repodata for environment", |_| async {
        gateway
            .query(channels, [args.platform, Platform::NoArch], specs.clone())
            .recursive(true)
            .execute()
            .await
            .into_diagnostic()
    })
    .await
    .context("failed to get repodata")?;

    // Determine virtual packages of the current platform
    let virtual_packages: Vec<GenericVirtualPackage> =
        VirtualPackages::detect(&VirtualPackageOverrides::from_env())
            .into_diagnostic()
            .context("failed to determine virtual packages")?
            .into_generic_virtual_packages()
            .collect();

    // Solve the environment
    tracing::info!(
        "creating environment in {}",
        dunce::canonicalize(prefix.root())
            .as_deref()
            .unwrap_or(prefix.root())
            .display()
    );
    let solve_result = wrap_in_progress("solving environment", || {
        Solver.solve(SolverTask {
            specs: specs.clone(),
            virtual_packages: virtual_packages.clone(),
            ..SolverTask::from_iter(&repodata.clone())
        })
    });

    let (solved_records, final_specs) = match solve_result {
        Ok(records) => (records, specs.to_vec()),
        Err(err) if has_guessed_package && !args.with.is_empty() => {
            // If solving failed and we guessed a package while using --with,
            // try again without the guessed package (last spec)
            let guessed_package_name = specs[specs.len() - 1]
                .name
                .as_ref()
                .map(|name| name.as_source())
                .unwrap_or("<unknown>");
            tracing::debug!(
                "Solver failed with guessed package '{}', retrying without it: {}",
                guessed_package_name,
                err
            );
            let records = wrap_in_progress("retrying solve without guessed package", || {
                Solver.solve(SolverTask {
                    specs: specs[..specs.len() - 1].to_vec(),
                    virtual_packages: virtual_packages.clone(),
                    ..SolverTask::from_iter(&repodata.clone())
                })
            })
            .into_diagnostic()
            .context("failed to solve environment even without guessed package")?;
            (records, specs[..specs.len() - 1].to_vec())
        }
        Err(err) => {
            return Err(err)
                .into_diagnostic()
                .context("failed to solve environment");
        }
    };

    // Force the initialization of the rayon thread pool to avoid implicit creation
    // by the Installer.
    LazyLock::force(&RAYON_INITIALIZE);

    // Install the environment
    Installer::new()
        .with_target_platform(args.platform)
        .with_download_client(client.clone())
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .clear_when_done(true)
                .finish(),
        )
        .with_package_cache(PackageCache::new(
            cache_dir.join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
        ))
        .install(prefix.root(), solved_records.records.clone())
        .await
        .into_diagnostic()
        .context("failed to create environment")?;

    write_guard.finish().await.into_diagnostic()?;

    if let Some(ref regex) = args.list {
        list_exec_environment(final_specs, solved_records, regex.clone())?;
    }

    Ok(prefix)
}

fn list_exec_environment(
    specs: Vec<MatchSpec>,
    solved_records: rattler_conda_types::SolverResult,
    regex: String,
) -> Result<(), miette::Error> {
    let regex = { if regex.is_empty() { None } else { Some(regex) } };
    let mut packages_to_output = solved_records
        .records
        .iter()
        .map(|record| {
            PackageToOutput::new(
                &record.package_record,
                specs
                    .clone()
                    .into_iter()
                    .filter_map(|spec| spec.name) // Extract the name if it exists
                    .collect_vec()
                    .contains(&record.package_record.name),
            )
        })
        .collect_vec();
    if let Some(ref regex) = regex {
        let regex = regex::Regex::new(regex).into_diagnostic()?;
        packages_to_output.retain(|package| regex.is_match(package.name.as_normalized()));
    }
    let output_message = if let Some(ref regex) = regex {
        format!(
            "The environment has {} packages filtered by regex `{}`:",
            console::style(packages_to_output.len()).bold(),
            regex
        )
    } else {
        format!(
            "The environment has {} packages:",
            console::style(packages_to_output.len()).bold()
        )
    };
    packages_to_output.sort_by(|a, b| a.name.cmp(&b.name));
    println!("{}", output_message);
    print_package_table(packages_to_output).into_diagnostic()?;
    Ok(())
}

/// This function is used to guess the package name from the command.
fn guess_package_spec(command: &str) -> MatchSpec {
    // Replace any illegal character with a dash.
    // TODO: In the future it would be cool if we look at all packages available
    //  and try to find the closest match.
    let command = command.replace(
        |c| !matches!(c, 'a'..='z'|'A'..='Z'|'0'..='9'|'-'|'_'|'.'),
        "-",
    );

    MatchSpec {
        name: Some(PackageName::from_str(&command).expect("all illegal characters were removed")),
        ..Default::default()
    }
}

/// Run the activation scripts of the prefix.
async fn run_activation(
    prefix: &Prefix,
) -> miette::Result<std::collections::HashMap<String, String>> {
    wrap_in_progress("running activation", move || prefix.run_activation()).await
}
