use crate::common::PixiControl;
use pixi::activation::CurrentEnvVarBehavior;

mod common;

#[tokio::test]
async fn test_pixi_only_env_activation() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.project().unwrap();
    let default_env = project.default_environment();

    let pixi_only_env = project
        .get_activated_environment_variables(&default_env, CurrentEnvVarBehavior::Exclude)
        .await
        .unwrap();

    std::env::set_var("DIRTY_VAR", "Dookie");

    assert!(pixi_only_env.get("CONDA_PREFIX").is_some());
    assert!(pixi_only_env.get("DIRTY_VAR").is_none());
    // PWD is not a pixi var, so it is not included in pixi_only.
    assert!(pixi_only_env.get("PWD").is_none());
}

#[tokio::test]
async fn test_full_env_activation() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.project().unwrap();
    let default_env = project.default_environment();

    std::env::set_var("DIRTY_VAR", "Dookie");

    let full_env = project
        .get_activated_environment_variables(&default_env, CurrentEnvVarBehavior::Include)
        .await
        .unwrap();
    assert!(full_env.get("CONDA_PREFIX").is_some());
    assert!(full_env.get("DIRTY_VAR").is_some());
    assert!(full_env.get("PWD").is_some());
}

#[cfg(target_family = "unix")]
#[tokio::test]
async fn test_clean_env_activation() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.project().unwrap();
    let default_env = project.default_environment();

    std::env::set_var("DIRTY_VAR", "Dookie");

    let clean_env = project
        .get_activated_environment_variables(&default_env, CurrentEnvVarBehavior::Clean)
        .await
        .unwrap();
    assert!(clean_env.get("CONDA_PREFIX").is_some());
    assert!(clean_env.get("DIRTY_VAR").is_none());

    // PWD is not a pixi var, but it is passed into a clean env.
    assert!(clean_env.get("PWD").is_some());
}
