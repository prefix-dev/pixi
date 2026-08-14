use crate::common::PixiControl;
use crate::setup_tracing;
use pixi_core::{
    activation::CurrentEnvVarBehavior, workspace::get_activated_environment_variables,
};

#[cfg(windows)]
const HOME: &str = "HOMEPATH";
#[cfg(unix)]
const HOME: &str = "HOME";

#[tokio::test(flavor = "current_thread")]
async fn test_pixi_only_env_activation() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let default_env = workspace.default_environment();

    let pixi_only_env = get_activated_environment_variables(
        workspace.env_vars(),
        &default_env,
        None,
        CurrentEnvVarBehavior::Exclude,
        None,
        false,
        false,
    )
    .await
    .unwrap();

    // SAFETY: `set_var` is only unsafe in a multi-threaded context
    // We enforce that this test runs on the current thread
    unsafe {
        std::env::set_var("DIRTY_VAR_1", "Dookie");
    }

    assert!(pixi_only_env.get("CONDA_PREFIX").is_some());
    assert!(pixi_only_env.get("DIRTY_VAR_1").is_none());
    // This is not a pixi var, so it is not included in pixi_only.
    assert!(pixi_only_env.get(HOME).is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn test_full_env_activation() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let default_env = project.default_environment();

    // SAFETY: `set_var` is only unsafe in a multi-threaded context
    // We enforce that this test runs on the current thread
    unsafe {
        std::env::set_var("DIRTY_VAR_2", "Dookie");
    }

    let full_env = get_activated_environment_variables(
        project.env_vars(),
        &default_env,
        None,
        CurrentEnvVarBehavior::Include,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(full_env.get("CONDA_PREFIX").is_some());
    assert!(full_env.get("DIRTY_VAR_2").is_some());
    assert!(full_env.get(HOME).is_some());
}

#[cfg(target_family = "unix")]
#[tokio::test(flavor = "current_thread")]
async fn test_clean_env_activation() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let default_env = project.default_environment();

    // SAFETY: `set_var` is only unsafe in a multi-threaded context
    // We enforce that this test runs on the current thread
    unsafe {
        std::env::set_var("DIRTY_VAR_3", "Dookie");
    }

    let clean_env = get_activated_environment_variables(
        project.env_vars(),
        &default_env,
        None,
        CurrentEnvVarBehavior::Clean,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(clean_env.get("CONDA_PREFIX").is_some());
    assert!(clean_env.get("DIRTY_VAR_3").is_none());

    // This is not a pixi var, but it is passed into a clean env.
    assert!(clean_env.get(HOME).is_some());
}

/// Regression tests for <https://github.com/prefix-dev/pixi/issues/6773>:
/// `[target.<custom-platform>.activation]` must be scoped to the platform a
/// run targets, not to whichever declared platform sorts first for this host.
mod custom_platform_scoping {
    use super::*;
    use pixi_core::activation::get_activator;
    use pixi_manifest::PixiPlatformName;
    use rattler_conda_types::Platform;
    use rattler_shell::shell::ShellEnum;

    /// A workspace with two custom platforms on the *current* subdir:
    /// `generic` (no extra virtual packages — always host-runnable, and
    /// declared first so it is the host's best declared platform) and
    /// `local` (declares `__cuda`, mimicking the issue's machine-specific
    /// entry). Each platform gets its own `[target.<name>.activation]`.
    fn manifest() -> String {
        let current = Platform::current();
        format!(
            r#"
            [workspace]
            name = "custom-platform-activation"
            channels = []
            platforms = [
                {{ name = "generic", platform = "{current}" }},
                {{ name = "local", platform = "{current}", cuda = "12" }},
            ]

            [activation.env]
            COMMON = "always"

            [target.local.activation.env]
            BAR = "foo"

            [target.generic.activation.env]
            ONLY_GENERIC = "yes"
            "#
        )
    }

    fn platform_name(name: &str) -> PixiPlatformName {
        PixiPlatformName::try_from(name).unwrap()
    }

    /// `pixi run --platform local` / `--platform generic` must each see only
    /// their own target's activation env (plus the default target's).
    #[tokio::test(flavor = "current_thread")]
    async fn test_target_activation_env_scoped_to_requested_platform() {
        setup_tracing();
        let pixi = PixiControl::from_manifest(&manifest()).unwrap();

        // Explicitly requested `local`: `[target.local]` applies, `[target.generic]` doesn't.
        {
            let workspace = pixi.workspace().unwrap();
            let env = workspace.default_environment();
            let local = env
                .named_or_best_declared_platform(Some(&platform_name("local")))
                .expect("local is declared by the default environment");
            let vars = get_activated_environment_variables(
                workspace.env_vars(),
                &env,
                Some(local),
                CurrentEnvVarBehavior::Exclude,
                None,
                false,
                false,
            )
            .await
            .unwrap();
            assert_eq!(
                vars.get("BAR").map(String::as_str),
                Some("foo"),
                "target.local activation env must apply when running for platform 'local'"
            );
            assert_eq!(
                vars.get("ONLY_GENERIC"),
                None,
                "target.generic activation env must not leak into platform 'local'"
            );
            assert_eq!(vars.get("COMMON").map(String::as_str), Some("always"));
        }

        // Explicitly requested `generic`: the mirror image.
        {
            let workspace = pixi.workspace().unwrap();
            let env = workspace.default_environment();
            let generic = env
                .named_or_best_declared_platform(Some(&platform_name("generic")))
                .expect("generic is declared by the default environment");
            let vars = get_activated_environment_variables(
                workspace.env_vars(),
                &env,
                Some(generic),
                CurrentEnvVarBehavior::Exclude,
                None,
                false,
                false,
            )
            .await
            .unwrap();
            assert_eq!(
                vars.get("BAR"),
                None,
                "target.local activation env must not leak into platform 'generic'"
            );
            assert_eq!(vars.get("ONLY_GENERIC").map(String::as_str), Some("yes"));
            assert_eq!(vars.get("COMMON").map(String::as_str), Some("always"));
        }
    }

    /// With no platform requested and nothing installed, activation resolves
    /// to the host's best declared platform (`generic`, the first entry).
    #[tokio::test(flavor = "current_thread")]
    async fn test_target_activation_env_defaults_to_best_declared_platform() {
        setup_tracing();
        let pixi = PixiControl::from_manifest(&manifest()).unwrap();
        let workspace = pixi.workspace().unwrap();
        let env = workspace.default_environment();
        let vars = get_activated_environment_variables(
            workspace.env_vars(),
            &env,
            None,
            CurrentEnvVarBehavior::Exclude,
            None,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(vars.get("ONLY_GENERIC").map(String::as_str), Some("yes"));
        assert_eq!(
            vars.get("BAR"),
            None,
            "the host cannot run 'local' (no __cuda), so its target must not apply"
        );
    }

    /// With no platform requested but the environment *installed* for a
    /// specific declared platform, activation must follow the installed
    /// platform — `pixi install --platform X` then `pixi run` may not apply a
    /// sibling platform's target.
    #[tokio::test(flavor = "current_thread")]
    async fn test_target_activation_env_follows_installed_platform() {
        setup_tracing();
        let pixi = PixiControl::from_manifest(&manifest()).unwrap();
        let workspace = pixi.workspace().unwrap();
        let env = workspace.default_environment();

        // Pretend the environment was installed for `local` by writing the
        // `conda-meta/pixi` marker an install would leave behind.
        let local = env
            .named_or_best_declared_platform(Some(&platform_name("local")))
            .expect("local is declared by the default environment");
        pixi_core::environment::write_environment_file(
            &env.dir(),
            pixi_core::environment::EnvironmentFile {
                manifest_path: pixi.manifest_path(),
                environment_name: env.name().to_string(),
                pixi_version: pixi_consts::consts::PIXI_VERSION.to_string(),
                environment_lock_file_hash: pixi_core::environment::LockedEnvironmentHash::invalid(
                ),
                resolved_platform: Some(local.into()),
                minimum_supported_platform: Some(local.into()),
                source_fingerprints: Default::default(),
            },
        )
        .unwrap();

        let vars = get_activated_environment_variables(
            workspace.env_vars(),
            &env,
            None,
            CurrentEnvVarBehavior::Exclude,
            None,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            vars.get("BAR").map(String::as_str),
            Some("foo"),
            "activation must follow the installed platform 'local'"
        );
        assert_eq!(
            vars.get("ONLY_GENERIC"),
            None,
            "the best declared platform 'generic' must not override the installed platform"
        );
    }

    /// `[target.<custom-platform>.activation] scripts` must be scoped the
    /// same way as the activation env.
    #[tokio::test(flavor = "current_thread")]
    async fn test_target_activation_scripts_scoped_to_requested_platform() {
        setup_tracing();
        let ext = if Platform::current().is_windows() {
            "bat"
        } else {
            "sh"
        };
        let current = Platform::current();
        let manifest = format!(
            r#"
            [workspace]
            name = "custom-platform-scripts"
            channels = []
            platforms = [
                {{ name = "generic", platform = "{current}" }},
                {{ name = "local", platform = "{current}", cuda = "12" }},
            ]

            [target.local.activation]
            scripts = ["local.{ext}"]

            [target.generic.activation]
            scripts = ["generic.{ext}"]
            "#
        );
        let pixi = PixiControl::from_manifest(&manifest).unwrap();
        fs_err::write(pixi.workspace_path().join(format!("local.{ext}")), "").unwrap();
        fs_err::write(pixi.workspace_path().join(format!("generic.{ext}")), "").unwrap();

        let workspace = pixi.workspace().unwrap();
        let env = workspace.default_environment();
        let local = env
            .named_or_best_declared_platform(Some(&platform_name("local")))
            .unwrap();
        let generic = env
            .named_or_best_declared_platform(Some(&platform_name("generic")))
            .unwrap();

        let script_names = |platform| {
            get_activator(&env, ShellEnum::default(), platform)
                .unwrap()
                .activation_scripts
                .into_iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(script_names(Some(local)), vec![format!("local.{ext}")]);
        assert_eq!(script_names(Some(generic)), vec![format!("generic.{ext}")]);
    }
}
