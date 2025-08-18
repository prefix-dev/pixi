use crate::common::PixiControl;
use pixi_core::{
    activation::CurrentEnvVarBehavior, workspace::get_activated_environment_variables,
};

#[cfg(windows)]
const HOME: &str = "HOMEPATH";
#[cfg(unix)]
const HOME: &str = "HOME";

#[tokio::test(flavor = "current_thread")]
async fn test_pixi_only_env_activation() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let workspace = pixi.workspace().unwrap();
    let default_env = workspace.default_environment();

    let pixi_only_env = get_activated_environment_variables(
        workspace.env_vars(),
        &default_env,
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
