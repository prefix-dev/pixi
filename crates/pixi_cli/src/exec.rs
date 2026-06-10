use std::{
    collections::BTreeSet,
    collections::HashMap,
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{Parser, ValueHint};
use indexmap::IndexSet;
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use pixi_api::workspace::platforms::resolve_platforms;
use pixi_config::{self, Config, ConfigCli};
use pixi_core::environment::list::{PackageToOutput, print_package_table};
use pixi_manifest::PixiPlatformName;
use pixi_progress::{await_in_progress, global_multi_progress, wrap_in_progress};
use pixi_script::{
    ScriptMetadata,
    lock::{self, ScriptLock},
};
use pixi_utils::prefix::Prefix;
use pixi_utils::{EnvironmentHash, EnvironmentLock, reqwest::build_reqwest_clients};
use rattler::{
    install::{IndicatifReporter, Installer},
    package_cache::PackageCache,
};
use rattler_conda_types::{
    Channel, GenericVirtualPackage, MatchSpec, NamedChannelOrUrl, PackageName, Platform,
    RepoDataRecord,
};
use rattler_solve::{SolverImpl, SolverTask, resolvo::Solver};
use rattler_virtual_packages::{VirtualPackageOverrides, VirtualPackages};
use reqwest_middleware::ClientWithMiddleware;
use uv_configuration::initialize_rayon_once;

use crate::{cli_config::ChannelsConfig, match_spec_or_path::MatchSpecOrPath, process_exit};

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
    pub specs: Vec<MatchSpecOrPath>,

    /// Matchspecs of package to install, while also guessing a package
    /// from the command.
    #[clap(long, short = 'w', conflicts_with = "specs")]
    pub with: Vec<MatchSpecOrPath>,

    #[clap(flatten)]
    channels: ChannelsConfig,

    /// The platform to create the environment for. Defaults to the
    /// current machine's subdir. Accepts a workspace platform name or a
    /// bare conda subdir (e.g. `linux-64`); `pixi exec` runs outside any
    /// workspace so the value resolves to a conda subdir either way.
    #[clap(long, short)]
    pub platform: Option<PixiPlatformName>,

    /// If specified a new environment is always created even if one already
    /// exists.
    #[clap(long)]
    pub force_reinstall: bool,

    /// When the command is a script with an inline metadata block, write (or
    /// refresh) a `<script>.pixi.lock` file next to it. Later runs create the
    /// environment from the lock file instead of solving, as long as the
    /// metadata has not changed. Combine with `--force-reinstall` to re-solve
    /// and refresh the lock file.
    #[clap(long, conflicts_with_all = ["specs", "with"])]
    pub lock: bool,

    /// Ignore an existing script lock file for this run. The lock file is
    /// neither read nor modified.
    #[clap(long, conflicts_with = "lock")]
    pub ignore_lock: bool,

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

    // `pixi exec` runs without a workspace, so the resolver only has the
    // bare-subdir fallback to work with. Anything that isn't a valid conda
    // subdir is rejected before we touch the network.
    let platform = match &args.platform {
        Some(name) => resolve_platforms(&IndexSet::default(), std::slice::from_ref(name))?
            .into_iter()
            .next()
            .expect("resolve_platforms preserves length")
            .subdir(),
        None => Platform::current(),
    };

    let mut command_iter = args.command.iter();
    let command = command_iter.next().ok_or_else(|| miette::miette!(help ="i.e when specifying specs explicitly use a command at the end: `pixi exec -s python==3.12 python`", "missing required command to execute",))?;
    let (_, client) = build_reqwest_clients(Some(&config), None)?;

    // When the command refers to an existing file, it may embed an inline
    // script metadata block that describes the environment it needs.
    let script_metadata = load_script_metadata(Path::new(command))?;
    if args.lock && script_metadata.is_none() {
        return Err(miette::miette!(
            help = "add a `# /// script` metadata block to the script, see `pixi exec --help`",
            "`--lock` requires the command to be a script with an inline metadata block",
        ));
    }

    // Determine the specs for installation and for the environment name.
    let exec_specs = to_exec_match_specs(&args.specs)?;
    let exec_with = to_exec_match_specs(&args.with)?;

    let mut install_specs = exec_specs.clone();
    install_specs.extend(exec_with.clone());

    let mut display_names: Vec<String> = args
        .specs
        .iter()
        .filter_map(|spec| spec.display_name())
        .collect();
    display_names.extend(args.with.iter().filter_map(|spec| spec.display_name()));

    // Use the dependencies embedded in the script unless `--spec` overrides
    // them; `--with` specs are installed alongside them.
    if let Some(metadata) = script_metadata.as_ref().filter(|_| exec_specs.is_empty()) {
        install_specs = metadata.dependencies(platform);
        install_specs.extend(exec_with.iter().cloned());
        display_names = install_specs
            .iter()
            .filter_map(|spec| spec.name.as_exact())
            .map(|name| name.as_source().to_string())
            .collect();
    }

    // Guess a package from the command if no specs were provided at all OR if
    // --with is used. A script with embedded metadata declares its
    // dependencies explicitly, so nothing is guessed from its file name.
    let should_guess_package =
        script_metadata.is_none() && (args.specs.is_empty() || !args.with.is_empty());
    if should_guess_package {
        install_specs.push(guess_package_spec(command));
    }

    // Channels given on the command line take precedence over channels from
    // the script metadata, which in turn beat the configured defaults.
    let channels = resolve_channels(
        &args,
        &config,
        script_metadata.as_ref().and_then(ScriptMetadata::channels),
    )?;

    // Look for the script's sidecar lock file, unless this run ignores it.
    let lock_state = match &script_metadata {
        Some(metadata) if !args.ignore_lock => {
            Some(ScriptLockState::load(Path::new(command), metadata)?)
        }
        _ => None,
    };

    // The lock file records a resolution of the script's own metadata, so it
    // only applies to runs without command line overrides.
    let cli_overrides =
        !args.specs.is_empty() || !args.with.is_empty() || args.channels.is_explicit();
    let locked_records = match &lock_state {
        Some(state) if !cli_overrides && !args.force_reinstall => {
            state.up_to_date_records(platform)?
        }
        _ => None,
    };

    // Create the environment to run the command in.
    let prefix = create_exec_prefix(
        &args,
        platform,
        EnvironmentSpec {
            specs: install_specs,
            channels: channels.clone(),
            locked_records,
        },
        &config,
        &client,
        should_guess_package,
    )
    .await?;

    // With `--lock`, record the installed environment in the sidecar lock
    // file so that later runs (and other machines) skip solving.
    if args.lock {
        let state = lock_state
            .as_ref()
            .expect("`--lock` requires script metadata");
        state.write(platform, &prefix, &channels)?;
    }

    // Get environment variables from the activation
    let mut activation_env = run_activation(&prefix).await?;

    // Collect unique package names for environment naming
    let package_names: BTreeSet<String> = display_names.into_iter().collect();

    if !package_names.is_empty() {
        let env_name = format!("temp:{}", package_names.into_iter().format(","));

        activation_env.insert("PIXI_ENVIRONMENT_NAME".into(), env_name.clone());

        if !args.no_modify_ps1 && std::env::current_dir().is_ok() {
            let (prompt_var, prompt_value) = if cfg!(windows) {
                ("_PIXI_PROMPT", format!("(pixi:{env_name}) $P$G"))
            } else {
                ("PS1", format!(r"(pixi:{env_name}) [\w] \$"))
            };

            activation_env.insert(prompt_var.into(), prompt_value);

            if cfg!(windows) {
                activation_env.insert("PROMPT".into(), String::from("$P$G"));
            }
        }
    }

    // Ignore CTRL+C so that the child is responsible for its own signal handling.
    let _ctrl_c = tokio::spawn(async { while tokio::signal::ctrl_c().await.is_ok() {} });

    // An entrypoint from the metadata block (e.g. `python`) wraps the script;
    // otherwise the command is spawned directly. Like conda-exec, a `.py`
    // script that declares a Python requirement but no entrypoint is run with
    // `python`.
    let entrypoint = script_metadata.as_ref().and_then(|metadata| {
        metadata.entrypoint(platform).or_else(|| {
            (metadata.requires_python()
                && Path::new(command)
                    .extension()
                    .is_some_and(|ext| ext == "py"))
            .then_some("python")
        })
    });
    let (program, program_args) = entrypoint_command_line(command, entrypoint, command_iter)?;

    // Spawn the command
    let mut cmd = std::process::Command::new(&program);
    cmd.args(&program_args);

    // On Windows, when using cmd.exe or cmd, we need to pass the full environment
    // because cmd.exe requires access to all environment variables (including prompt variables)
    // to properly display the modified prompt
    if cfg!(windows) && (program.to_lowercase().ends_with("cmd.exe") || program == "cmd") {
        let mut env = std::env::vars().collect::<HashMap<String, String>>();
        env.extend(activation_env);
        cmd.envs(env);
    } else {
        cmd.envs(activation_env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    let status = cmd.status().into_diagnostic().with_context(|| {
        if script_metadata.is_some() && entrypoint.is_none() {
            format!(
                "failed to execute '{program}', the script metadata block defines no \
                 `entrypoint`; add one under `[tool.pixi]` (e.g. `entrypoint = \"python\"`) or \
                 make the script itself executable"
            )
        } else {
            format!("failed to execute '{program}'")
        }
    })?;

    // Mirror the child's exit (including signal deaths like SIGSEGV) so the
    // parent shell sees the same outcome it would if the child had run
    // directly.
    process_exit::exit_with_status(status);
}

/// Reads the inline script metadata block from `command` when it points to an
/// existing file. A file without a metadata block (or one that cannot be read
/// as UTF-8 text, such as a binary) yields `None`; a file with a malformed
/// block is an error.
fn load_script_metadata(command: &Path) -> miette::Result<Option<ScriptMetadata>> {
    if !command.is_file() {
        return Ok(None);
    }
    let Ok(source) = fs_err::read_to_string(command) else {
        return Ok(None);
    };
    let metadata = ScriptMetadata::from_source(&source)
        .into_diagnostic()
        .with_context(|| {
            format!(
                "failed to parse the inline metadata block in '{}'",
                command.display()
            )
        })?;
    if metadata.is_some() {
        tracing::info!("using inline script metadata from {}", command.display());
    }
    Ok(metadata)
}

/// The sidecar lock file that belongs to the script being executed.
struct ScriptLockState {
    /// Where the lock file lives: `<script>.pixi.lock` next to the script.
    path: PathBuf,
    /// The digest of the script's current metadata block.
    input_hash: String,
    /// The lock file as it exists on disk, when present.
    existing: Option<ScriptLock>,
}

impl ScriptLockState {
    /// Reads the lock file belonging to `script`, when there is one.
    fn load(script: &Path, metadata: &ScriptMetadata) -> miette::Result<Self> {
        let path = lock::lock_path(script);
        let existing = ScriptLock::read(&path).into_diagnostic()?;
        Ok(Self {
            path,
            input_hash: lock::input_hash(metadata.document()),
            existing,
        })
    }

    /// The locked records to install, when the lock file on disk matches the
    /// script's current metadata and covers `platform`.
    fn up_to_date_records(
        &self,
        platform: Platform,
    ) -> miette::Result<Option<Vec<RepoDataRecord>>> {
        let Some(lock) = &self.existing else {
            return Ok(None);
        };
        if !lock.is_up_to_date(&self.input_hash) {
            tracing::warn!(
                "the lock file {} does not match the script metadata anymore, re-solving; \
                 run with `--lock` to refresh it",
                self.path.display()
            );
            return Ok(None);
        }
        match lock.records(platform, &self.path).into_diagnostic()? {
            Some(records) => {
                tracing::info!(
                    "creating the environment from the lock file {}",
                    self.path.display()
                );
                Ok(Some(records))
            }
            None => {
                tracing::warn!(
                    "the lock file {} does not cover {platform}, re-solving; \
                     run with `--lock` to add it",
                    self.path.display()
                );
                Ok(None)
            }
        }
    }

    /// Records the packages installed in `prefix` in the lock file. The
    /// resolutions of other platforms are kept when the existing lock file
    /// matches the script's current metadata.
    fn write(
        &self,
        platform: Platform,
        prefix: &Prefix,
        channels: &IndexSet<Channel>,
    ) -> miette::Result<()> {
        let mut records: Vec<RepoDataRecord> = prefix
            .find_installed_packages()
            .into_diagnostic()
            .context("failed to read the installed packages from the prefix")?
            .into_iter()
            .map(|record| record.repodata_record)
            .collect();
        records.sort_by(|a, b| a.package_record.name.cmp(&b.package_record.name));

        ScriptLock::write(
            &self.path,
            &self.input_hash,
            platform,
            &records,
            channels.iter().map(|channel| channel.base_url.to_string()),
            self.existing
                .as_ref()
                .filter(|lock| lock.is_up_to_date(&self.input_hash)),
        )
        .into_diagnostic()?;

        eprintln!(
            "{}Wrote lock file {}",
            console::style(console::Emoji("✔ ", "")).green(),
            self.path.display()
        );
        Ok(())
    }
}

/// Builds the program and argument list to spawn. An `entrypoint` from the
/// script metadata is split on whitespace and any `${SCRIPT}` placeholder
/// in it is replaced by the script path; without a placeholder the script
/// path is appended after the entrypoint instead. The remaining command line
/// arguments are passed through in both cases.
fn entrypoint_command_line<'a>(
    command: &str,
    entrypoint: Option<&str>,
    extra_args: impl Iterator<Item = &'a String>,
) -> miette::Result<(String, Vec<String>)> {
    let Some(entrypoint) = entrypoint else {
        return Ok((command.to_string(), extra_args.cloned().collect()));
    };

    let expanded = entrypoint.replace("${SCRIPT}", command);
    let mut parts = expanded.split_whitespace().map(str::to_string);
    let program = parts
        .next()
        .ok_or_else(|| miette::miette!("the `entrypoint` in the script metadata block is empty"))?;

    let mut args: Vec<String> = parts.collect();
    if !entrypoint.contains("${SCRIPT}") {
        args.push(command.to_string());
    }
    args.extend(extra_args.cloned());
    Ok((program, args))
}

/// Resolves the channels for the temporary environment. Channels given on the
/// command line take precedence over channels from the script metadata; when
/// neither is present the configured default channels are used.
fn resolve_channels(
    args: &Args,
    config: &Config,
    script_channels: Option<&[NamedChannelOrUrl]>,
) -> miette::Result<IndexSet<Channel>> {
    if let Some(channels) = script_channels.filter(|_| !args.channels.is_explicit()) {
        return channels
            .iter()
            .map(|channel| channel.clone().into_channel(config.global_channel_config()))
            .try_collect()
            .into_diagnostic();
    }
    args.channels.resolve_from_config(config)
}

/// What the temporary environment of `pixi exec` must contain.
pub struct EnvironmentSpec {
    /// The match specs to solve the environment from.
    pub specs: Vec<MatchSpec>,
    /// The channels to solve against.
    pub channels: IndexSet<Channel>,
    /// Records from an up-to-date script lock file. When present they are
    /// installed verbatim instead of solving `specs`.
    pub locked_records: Option<Vec<RepoDataRecord>>,
}

/// Creates a prefix for the `pixi exec` command.
pub async fn create_exec_prefix(
    args: &Args,
    platform: Platform,
    environment: EnvironmentSpec,
    config: &Config,
    client: &ClientWithMiddleware,
    has_guessed_package: bool,
) -> miette::Result<Prefix> {
    let cache_dir = pixi_config::get_cache_dir().context("failed to determine cache directory")?;
    let EnvironmentSpec {
        specs,
        channels,
        locked_records,
    } = environment;

    let environment_hash = EnvironmentHash::new(
        specs.clone(),
        channels.iter().map(|c| c.base_url.to_string()).collect(),
        platform,
    );

    let dir_prefix = exec_dir_prefix(
        &specs,
        args.command.first().map(String::as_str),
        has_guessed_package,
    );

    let prefix = Prefix::new(
        cache_dir
            .join(pixi_consts::consts::CACHED_ENVS_DIR)
            .join(environment_hash.name(dir_prefix.as_deref())),
    );

    // Cross-process install lock. The prefix is content-addressed by
    // `environment_hash`, so any prior finish here is reusable.
    let mut env_lock = await_in_progress("acquiring write lock on prefix", |_| {
        EnvironmentLock::acquire(prefix.root())
    })
    .await
    .into_diagnostic()
    .context("failed to acquire write lock on prefix")?;

    // Reuse the cached prefix when it is already installed. `--list`
    // still needs the solved records to print the table, so it falls
    // through to the (cheap, no-op) install path below rather than
    // returning here.
    if !args.force_reinstall && args.list.is_none() && env_lock.current().is_some() {
        tracing::info!(
            "reusing existing environment in {}",
            prefix.root().display()
        );
        return Ok(prefix);
    }

    // A previous install here crashed; re-link everything below.
    let reinstall_all = env_lock.was_interrupted();

    tracing::info!(
        "creating environment in {}",
        dunce::canonicalize(prefix.root())
            .as_deref()
            .unwrap_or(prefix.root())
            .display()
    );

    // Records from an up-to-date script lock file are installed verbatim;
    // otherwise the environment is solved from the specs.
    let (solved_records, final_specs) = match locked_records {
        Some(records) => (records, specs.clone()),
        None => {
            solve_environment(
                args,
                platform,
                &specs,
                channels,
                config,
                client,
                has_guessed_package,
            )
            .await?
        }
    };

    // Force the initialization of the rayon thread pool to avoid implicit creation
    // by the Installer.
    initialize_rayon_once();

    // Mark the prefix dirty for the duration of the install so a crash
    // is detected next time.
    env_lock
        .begin()
        .await
        .into_diagnostic()
        .context("failed to mark prefix install in progress")?;

    // Install the environment. When recovering from an interrupted
    // install, re-link every package rather than trusting conda-meta.
    let mut installer = Installer::new()
        .with_target_platform(platform)
        .with_download_client(client.clone())
        .with_reporter(
            IndicatifReporter::builder()
                .with_multi_progress(global_multi_progress())
                .clear_when_done(true)
                .finish(),
        )
        .with_package_cache(PackageCache::new(
            cache_dir.join(pixi_consts::consts::CONDA_PACKAGE_CACHE_DIR),
        ));
    if reinstall_all {
        installer = installer.with_reinstall_packages(
            solved_records
                .iter()
                .map(|r| r.package_record.name.clone())
                .collect(),
        );
    }
    installer
        .install(prefix.root(), solved_records.clone())
        .await
        .into_diagnostic()
        .context("failed to create environment")?;

    let installed_fingerprint = pixi_utils::EnvironmentFingerprint::compute(solved_records.iter());
    env_lock
        .finish(&installed_fingerprint)
        .await
        .into_diagnostic()
        .context("failed to record prefix install fingerprint")?;

    if let Some(ref regex) = args.list {
        list_exec_environment(final_specs, solved_records, regex.clone())?;
    }

    Ok(prefix)
}

/// Fetches repodata and solves the environment for `specs`. When solving
/// fails and the last spec was guessed from a command used with `--with`, the
/// solve is retried without the guessed spec. Returns the solved records and
/// the specs that produced them.
async fn solve_environment(
    args: &Args,
    platform: Platform,
    specs: &[MatchSpec],
    channels: IndexSet<Channel>,
    config: &Config,
    client: &ClientWithMiddleware,
    has_guessed_package: bool,
) -> miette::Result<(Vec<RepoDataRecord>, Vec<MatchSpec>)> {
    // Construct a gateway to get repodata.
    let gateway = config.gateway().with_client(client.clone()).finish();

    // Get the repodata for the specs
    let repodata = await_in_progress("fetching repodata for environment", |_| async {
        gateway
            .query(channels, [platform, Platform::NoArch], specs.to_vec())
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

    let solve_result = wrap_in_progress("solving environment", || {
        Solver.solve(SolverTask {
            specs: specs.to_vec(),
            virtual_packages: virtual_packages.clone(),
            ..SolverTask::from_iter(&repodata.clone())
        })
    });

    match solve_result {
        Ok(result) => Ok((result.records, specs.to_vec())),
        Err(err) if has_guessed_package && !args.with.is_empty() => {
            // If solving failed and we guessed a package while using --with,
            // try again without the guessed package (last spec)
            let guessed_package_name = specs[specs.len() - 1]
                .name
                .as_exact()
                .map(|n| n.as_source())
                .unwrap_or("<unknown>");
            tracing::debug!(
                "Solver failed with guessed package '{}', retrying without it: {}",
                guessed_package_name,
                err
            );
            let result = wrap_in_progress("retrying solve without guessed package", || {
                Solver.solve(SolverTask {
                    specs: specs[..specs.len() - 1].to_vec(),
                    virtual_packages: virtual_packages.clone(),
                    ..SolverTask::from_iter(&repodata.clone())
                })
            })
            .into_diagnostic()
            .context("failed to solve environment even without guessed package")?;
            Ok((result.records, specs[..specs.len() - 1].to_vec()))
        }
        Err(err) => Err(err)
            .into_diagnostic()
            .context("failed to solve environment"),
    }
}

fn list_exec_environment(
    specs: Vec<MatchSpec>,
    solved_records: Vec<RepoDataRecord>,
    regex: String,
) -> Result<(), miette::Error> {
    let regex = { if regex.is_empty() { None } else { Some(regex) } };
    let mut packages_to_output = solved_records
        .iter()
        .map(|record| {
            PackageToOutput::new(
                &record.package_record,
                specs
                    .clone()
                    .into_iter()
                    .filter_map(|spec| spec.name.as_exact().cloned()) // Extract exact name if it exists
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
    println!("{output_message}");
    print_package_table(packages_to_output).into_diagnostic()?;
    Ok(())
}

/// Picks the human-readable prefix for the cached env directory:
/// the single spec's name when there is exactly one, otherwise the guessed
/// package (when pixi guessed one), otherwise nothing.
fn exec_dir_prefix(
    specs: &[MatchSpec],
    command: Option<&str>,
    has_guessed_package: bool,
) -> Option<String> {
    if let [single] = specs {
        return single
            .name
            .as_exact()
            .map(|name| name.as_normalized().to_string());
    }
    if has_guessed_package {
        return command.and_then(|c| {
            guess_package_spec(c)
                .name
                .as_exact()
                .map(|name| name.as_normalized().to_string())
        });
    }
    None
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
        name: PackageName::from_str(&command)
            .expect("all illegal characters were removed")
            .into(),
        ..Default::default()
    }
}

/// Run the activation scripts of the prefix.
async fn run_activation(
    prefix: &Prefix,
) -> miette::Result<std::collections::HashMap<String, String>> {
    wrap_in_progress("running activation", move || prefix.run_activation()).await
}

fn to_exec_match_specs(specs: &[MatchSpecOrPath]) -> miette::Result<Vec<MatchSpec>> {
    specs
        .iter()
        .cloned()
        .map(|spec| {
            spec.into_exec_match_spec()
                .map_err(|err| miette::miette!(err))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rattler_conda_types::{MatchSpec, ParseStrictness};

    use super::{entrypoint_command_line, exec_dir_prefix};

    fn spec(s: &str) -> MatchSpec {
        MatchSpec::from_str(s, ParseStrictness::Lenient).unwrap()
    }

    // `pixi exec --spec foo cmd`: single spec → use the spec name.
    #[test]
    fn single_explicit_spec_wins() {
        let prefix = exec_dir_prefix(&[spec("rucio-mcp")], Some("sh"), false);
        assert_eq!(prefix.as_deref(), Some("rucio-mcp"));
    }

    // `pixi exec cmd`: no spec, package guessed from cmd → use cmd.
    #[test]
    fn guessed_only_uses_command() {
        let prefix = exec_dir_prefix(&[spec("voms-proxy-init")], Some("voms-proxy-init"), true);
        assert_eq!(prefix.as_deref(), Some("voms-proxy-init"));
    }

    // `pixi exec --with extra cmd`: guess + extra → use cmd, not "extra".
    #[test]
    fn with_uses_command_not_extra_spec() {
        let prefix = exec_dir_prefix(&[spec("extra"), spec("cmd")], Some("cmd"), true);
        assert_eq!(prefix.as_deref(), Some("cmd"));
    }

    // `pixi exec --spec a --spec b cmd`: multiple explicit specs, no guess → no prefix.
    #[test]
    fn multiple_explicit_specs_have_no_prefix() {
        let prefix = exec_dir_prefix(&[spec("foo"), spec("bar")], Some("cmd"), false);
        assert_eq!(prefix, None);
    }

    // A bare entrypoint wraps the script; remaining CLI arguments follow.
    #[test]
    fn entrypoint_wraps_script_and_arguments() {
        let extra = vec!["--verbose".to_string()];
        let (program, args) =
            entrypoint_command_line("script.py", Some("python"), extra.iter()).unwrap();
        assert_eq!(program, "python");
        assert_eq!(args, ["script.py", "--verbose"]);
    }

    // A `${SCRIPT}` placeholder positions the script inside the entrypoint.
    #[test]
    fn entrypoint_script_placeholder_is_substituted() {
        let extra: Vec<String> = vec![];
        let (program, args) =
            entrypoint_command_line("script.sh", Some("bash -e ${SCRIPT}"), extra.iter()).unwrap();
        assert_eq!(program, "bash");
        assert_eq!(args, ["-e", "script.sh"]);
    }

    // Without an entrypoint the command is spawned as-is.
    #[test]
    fn without_entrypoint_the_command_is_spawned_directly() {
        let extra = vec!["arg".to_string()];
        let (program, args) = entrypoint_command_line("tool", None, extra.iter()).unwrap();
        assert_eq!(program, "tool");
        assert_eq!(args, ["arg"]);
    }

    #[test]
    fn empty_entrypoint_is_an_error() {
        let extra: Vec<String> = vec![];
        assert!(entrypoint_command_line("script.py", Some("  "), extra.iter()).is_err());
    }
}
