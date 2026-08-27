use std::{collections::HashMap, ffi::OsString, path::Path};

use deno_task_shell::KillSignal;
use miette::{IntoDiagnostic, NamedSource, Report};
use pixi_core::{
    Workspace,
    environment::sanity_check_workspace,
    lock_file::{ReinstallPackages, UpdateLockFileOptions, UpdateMode},
    workspace::virtual_packages::{
        EnvironmentRunnability, classify_environment_runnability,
        verify_current_platform_can_run_environment, verify_run_platform,
    },
};
use pixi_manifest::WithWarnings;
use pixi_manifest::script::conda::{CondaScriptError, CondaScriptManifest};
use pixi_script_shell::{ShellContext, execute_sequence, parse_sequence};
use pixi_task::get_task_env;
use tracing::Level;

use crate::{
    process_exit,
    run::{Args, run_future_forwarding_signals},
    shared::install_platform::resolve_install_platform,
};

/// Reads the `conda-script` block of a local `--script` file.
///
/// Returns `Ok(None)` when the file has no block or when a malformed block
/// appears in a Python file, so the caller falls back to the PEP 723 path: a
/// Python script may contain an accidental line ending in the opening
/// marker, say inside an indented docstring, and must keep working as it did
/// before the conda-script format existed. When `surface_errors` is set (the
/// caller passed `--experimental`) or the file cannot be a PEP 723 script
/// anyway, a block error is reported instead.
pub(crate) fn detect_with_fallback(
    path: &Path,
    surface_errors: bool,
) -> miette::Result<Option<CondaScriptManifest>> {
    match CondaScriptManifest::from_path(path) {
        Ok(manifest) => Ok(manifest),
        Err(error @ CondaScriptError::Io(_)) => Err(Report::new(error)),
        Err(error) => {
            let is_python = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("py") || extension.eq_ignore_ascii_case("pyw")
                });
            if surface_errors || !is_python {
                Err(Report::new(error))
            } else {
                tracing::debug!(
                    "ignoring a malformed conda-script block in {}: {error}",
                    path.display()
                );
                Ok(None)
            }
        }
    }
}

/// Whether the contents carry a conda-script block, well-formed or not.
///
/// Transient script sources use this to explain that conda-script files only
/// run from local paths, instead of reporting a missing PEP 723 block.
pub(crate) fn looks_like_conda_script(contents: &[u8]) -> bool {
    !matches!(
        CondaScriptManifest::from_source("conda-script-probe", contents),
        Ok(None)
    )
}

/// Solves and installs the environment of a `conda-script` file, then runs
/// its entrypoint through the mini-shell with the CLI arguments appended.
pub(crate) async fn execute_run(
    manifest: CondaScriptManifest,
    args: Args,
    config: pixi_config::Config,
) -> miette::Result<()> {
    let script_path = manifest.path().to_owned();
    let entrypoint = manifest.metadata().entrypoint.clone();

    let WithWarnings {
        value: workspace,
        warnings,
    } = Workspace::from_conda_script(manifest, config)?;
    for warning in warnings {
        tracing::warn!("{warning}");
    }
    sanity_check_workspace(&workspace).await?;

    let environment = workspace.default_environment();
    let allow_installs = args.lock_and_install_config.allow_installs();
    let user_platform = resolve_install_platform(&workspace, args.platform.as_ref())?;
    let run_platform = user_platform
        .clone()
        .or_else(|| environment.installed_resolved_platform_name());
    let best_declared_platform = environment.named_or_best_declared_platform(run_platform.as_ref());
    if allow_installs
        && best_declared_platform.is_none()
        && let Some(name) = user_platform.as_ref()
    {
        return Err(miette::miette!(
            "platform '{}' is not part of environment '{}'",
            name,
            environment.name(),
        ));
    }
    if allow_installs {
        environment.emit_emulation_warning();
    }

    // Select and parse the entrypoint before solving, so a syntax error or a
    // missing platform key surfaces without waiting for the environment.
    let activation_platform = best_declared_platform
        .cloned()
        .unwrap_or_else(|| environment.activation_platform());
    let subdir = activation_platform.subdir();
    let Some(command) = entrypoint.select(subdir) else {
        return Err(miette::miette!(
            help = "add a matching key to the `entrypoint` table, for example `unix`, `win` or the exact platform",
            "the entrypoint has no command for platform '{subdir}'"
        ));
    };
    let sequence = parse_sequence(command).map_err(|error| {
        Report::new(error).with_source_code(NamedSource::new("entrypoint", command.to_owned()))
    })?;

    let progress = pixi_reporters::TopLevelProgress::from_global();
    let mut lock_file = workspace
        .resolve_lock_file(
            Some(progress.clone()),
            UpdateLockFileOptions {
                lock_file_usage: args.lock_and_install_config.lock_file_usage()?,
                no_install: args.lock_and_install_config.no_install(),
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
                ..Default::default()
            },
        )
        .await?
        .0;
    lock_file.target_platform = user_platform.clone();

    if allow_installs && user_platform.is_none() {
        let runnability =
            classify_environment_runnability(&environment, Some(lock_file.as_lock_file()));
        if runnability == EnvironmentRunnability::Unsupported {
            return Err(
                match verify_current_platform_can_run_environment(
                    &environment,
                    Some(lock_file.as_lock_file()),
                ) {
                    Err(err) => err.into(),
                    Ok(()) => environment.unsupported_platform_error().into(),
                },
            );
        }
    }

    let print_command = |command: &str| {
        if tracing::enabled!(Level::WARN) {
            let file_name = script_path
                .file_name()
                .expect("an absolute script path always has a file name")
                .to_string_lossy();
            pixi_progress::println!(
                "{}{}{}{}{}",
                console::Emoji("✨ ", ""),
                console::style("Pixi script (").bold(),
                console::style(file_name).green().bold(),
                console::style("): ").bold(),
                command,
            );
        }
    };

    // A dry run stops after solving: nothing gets installed or executed.
    if args.dry_run {
        progress.on_clear();
        lock_file.command_dispatcher.clear_filesystem_caches().await;
        pixi_progress::println!(
            "{}{}\n\n",
            console::Emoji("🌵 ", ""),
            console::style("Dry-run mode enabled - no tasks will be executed.")
                .yellow()
                .bold()
        );
        print_command(command);
        return Ok(());
    }

    if allow_installs {
        lock_file
            .prefix(
                &environment,
                UpdateMode::QuickValidate,
                &ReinstallPackages::default(),
                &pixi_core::environment::InstallFilter::default(),
            )
            .await?;
        verify_run_platform(&environment, user_platform.as_ref())?;
    }
    progress.on_clear();
    lock_file.command_dispatcher.clear_filesystem_caches().await;

    let command_env = get_task_env(
        &environment,
        &activation_platform,
        args.clean_env,
        Some(lock_file.as_lock_file()),
        workspace.config().force_activate(),
        workspace.config().experimental_activation_cache_usage(),
    )
    .await?;

    print_command(command);

    let cache_dir = workspace.pixi_dir().join("cache");
    fs_err::create_dir_all(&cache_dir).into_diagnostic()?;
    let script = script_path
        .into_os_string()
        .into_string()
        .map_err(|_| miette::miette!("the script path must contain only valid UTF-8 characters"))?;
    let cache = cache_dir
        .into_os_string()
        .into_string()
        .map_err(|_| miette::miette!("the cache path must contain only valid UTF-8 characters"))?;

    // Signals pixi receives reach the entrypoint's processes, and the guard
    // terminates them when pixi stops for any other reason.
    let kill_signal = KillSignal::default();
    let _drop_guard = kill_signal.clone().drop_guard();
    let context = ShellContext {
        variables: HashMap::from([("SCRIPT".to_owned(), script), ("CACHE".to_owned(), cache)]),
        env: command_env
            .into_iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect(),
        cwd: std::env::current_dir().into_diagnostic()?,
        kill_signal: kill_signal.clone(),
    };
    let code = run_future_forwarding_signals(
        kill_signal,
        execute_sequence(&sequence, &args.task, &context),
    )
    .await
    .map_err(Report::new)?;
    if code != 0 {
        process_exit::exit_with_code(code);
    }
    Ok(())
}
