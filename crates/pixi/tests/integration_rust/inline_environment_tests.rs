//! Integration tests for inline environment configuration.
//!
//! Tests verify that environments can define dependencies and other feature
//! configuration directly, without needing explicit feature definitions.

use pixi_manifest::{FeatureName, HasFeaturesIter, HasWorkspaceManifest, TaskName};
use rattler_conda_types::Platform;

use crate::common::{LockFileExt, PixiControl};
use crate::setup_tracing;

/// Test that inline environment dependencies are parsed and resolved correctly.
#[tokio::test]
async fn test_inline_environment_dependencies() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
        [workspace]
        name = "test-inline-env"
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

        [environments.dev.dependencies]
        git = "*"
        "#,
    )
    .unwrap();

    // Verify the workspace was created correctly
    let workspace = pixi.workspace().unwrap();
    let manifest = (&workspace).workspace_manifest();

    // Check that a synthetic feature was created with the environment name
    assert!(
        manifest.features.contains_key(&FeatureName::from("dev")),
        "Synthetic feature 'dev' should be created"
    );

    // Check that the environment exists
    let env = workspace.environment("dev");
    assert!(env.is_some(), "Environment 'dev' should exist");

    // Verify the environment has the synthetic feature in its feature list
    let env = env.unwrap();
    assert!(
        env.features().any(|f| f.name == FeatureName::from("dev")),
        "Environment should reference the synthetic 'dev' feature"
    );
}

/// Test inline environment with both explicit features and inline config.
#[tokio::test]
async fn test_inline_environment_with_explicit_features() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
        [workspace]
        name = "test-inline-with-features"
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

        [feature.python.dependencies]
        python = "3.11.*"

        [environments.dev]
        features = ["python"]

        [environments.dev.dependencies]
        git = "*"
        "#,
    )
    .unwrap();

    let workspace = pixi.workspace().unwrap();
    let manifest = (&workspace).workspace_manifest();

    // Both features should exist
    assert!(manifest.features.contains_key(&FeatureName::from("python")));
    assert!(manifest.features.contains_key(&FeatureName::from("dev")));

    // The environment should have both features
    let env = workspace.environment("dev").unwrap();
    let feature_names: Vec<_> = env.features().map(|f| f.name.clone()).collect();

    assert!(
        feature_names.contains(&FeatureName::from("dev")),
        "Environment should have synthetic 'dev' feature"
    );
    assert!(
        feature_names.contains(&FeatureName::from("python")),
        "Environment should have explicit 'python' feature"
    );
}

/// Test that inline environment tasks work correctly.
#[tokio::test]
async fn test_inline_environment_tasks() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
        [workspace]
        name = "test-inline-tasks"
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

        [environments.dev.dependencies]
        git = "*"

        [environments.dev.tasks]
        hello = "echo hello"
        "#,
    )
    .unwrap();

    let workspace = pixi.workspace().unwrap();
    let manifest = (&workspace).workspace_manifest();

    // Check that the synthetic feature has the task
    let dev_feature = manifest
        .features
        .get(&FeatureName::from("dev"))
        .expect("dev feature should exist");

    assert!(
        dev_feature
            .targets
            .default()
            .tasks
            .contains_key(&TaskName::from("hello")),
        "Synthetic feature should have the 'hello' task"
    );
}

/// Test that lock file is generated correctly for inline environment.
#[tokio::test]
async fn test_inline_environment_lock_file() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
        [workspace]
        name = "test-inline-lock"
        channels = ["https://prefix.dev/conda-forge"]
        platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

        [environments.dev.dependencies]
        zlib = "*"
        "#,
    )
    .unwrap();

    // Update the lock file
    let lock = pixi.update_lock_file().await.unwrap();

    // Verify the environment exists in the lock file and has the dependency
    assert!(
        lock.contains_conda_package("dev", Platform::current(), "zlib"),
        "Lock file should contain 'zlib' for 'dev' environment"
    );
}
