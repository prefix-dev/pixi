use crate::common::PixiControl;
use pixi::{activation::CurrentEnvVarBehavior, workspace::get_activated_environment_variables};

#[cfg(windows)]
const HOME: &str = "HOMEPATH";
#[cfg(unix)]
const HOME: &str = "HOME";

#[tokio::test]
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

    std::env::set_var("DIRTY_VAR", "Dookie");

    assert!(pixi_only_env.get("CONDA_PREFIX").is_some());
    assert!(pixi_only_env.get("DIRTY_VAR").is_none());
    // This is not a pixi var, so it is not included in pixi_only.
    assert!(pixi_only_env.get(HOME).is_none());
}

#[tokio::test]
async fn test_full_env_activation() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let default_env = project.default_environment();

    std::env::set_var("DIRTY_VAR", "Dookie");

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
    assert!(full_env.get("DIRTY_VAR").is_some());
    assert!(full_env.get(HOME).is_some());
}

#[cfg(target_family = "unix")]
#[tokio::test]
async fn test_clean_env_activation() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let default_env = project.default_environment();

    std::env::set_var("DIRTY_VAR", "Dookie");

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
    assert!(clean_env.get("DIRTY_VAR").is_none());

    // This is not a pixi var, but it is passed into a clean env.
    assert!(clean_env.get(HOME).is_some());
}
