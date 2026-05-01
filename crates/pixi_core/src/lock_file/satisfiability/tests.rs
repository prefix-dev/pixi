// Top-level orchestration tests for satisfiability verification. They walk
// through the full pipeline against fixture workspaces and snapshot the
// resulting diagnostic. Lives at `lock_file::satisfiability::tests` so the
// existing snapshot files under `satisfiability/snapshots/` keep matching
// the generated module-path key. Per-module unit tests live next to the
// code they exercise.

use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Component, PathBuf},
    sync::Arc,
};

use dashmap::DashMap;
use insta::Settings;
use itertools::Itertools;
use miette::{Diagnostic, IntoDiagnostic, NarratableReportHandler};
use once_cell::sync::OnceCell;
use pep440_rs::{Operator, Version, VersionSpecifiers};
use pixi_build_backend_passthrough::PassthroughBackend;
use pixi_build_frontend::BackendOverride;
use pixi_command_dispatcher::{CacheDirs, CommandDispatcherError};
use pixi_manifest::FeaturesExt;
use pixi_record::LockFileResolver;
use pixi_uv_context::UvResolutionContext;
use rattler_conda_types::Platform;
use rattler_lock::LockFile;
use rstest::rstest;
use std::str::FromStr;
use thiserror::Error;
use tracing_test::traced_test;

use super::{
    EnvironmentUnsat, PlatformUnsat, SolveGroupUnsat, VerifySatisfiabilityContext, pypi_metadata,
    verify_environment_satisfiability, verify_platform_satisfiability,
    verify_solve_group_satisfiability,
};
use crate::{
    Workspace,
    lock_file::outdated::{BuildCacheKey, PypiEnvironmentBuildCache},
};

#[derive(Error, Debug, Diagnostic)]
enum LockfileUnsat {
    #[error("environment '{0}' is missing")]
    EnvironmentMissing(String),

    #[error("environment '{0}' does not satisfy the requirements of the project")]
    Environment(String, #[source] EnvironmentUnsat),

    #[error(
        "environment '{0}' does not satisfy the requirements of the project for platform '{1}'"
    )]
    PlatformUnsat(String, Platform, #[source] PlatformUnsat),

    #[error(
        "solve group '{0}' does not satisfy the requirements of the project for platform '{1}'"
    )]
    SolveGroupUnsat(String, Platform, #[source] SolveGroupUnsat),

    #[error("failed to build the lock-file resolver: {0}")]
    ResolverBuild(String),
}

async fn verify_lockfile_satisfiability(
    project: &Workspace,
    lock_file: &LockFile,
    backend_override: Option<BackendOverride>,
) -> Result<(), LockfileUnsat> {
    // Ensure the rayon thread pool is initialized before any code path
    // that might trigger implicit rayon initialization (e.g. uv's
    // DistributionDatabase). Without this, concurrent tests can race
    // and trigger a GlobalPoolAlreadyInitialized panic.
    std::sync::LazyLock::force(&uv_configuration::RAYON_INITIALIZE);

    let mut individual_verified_envs = HashMap::new();

    let temp_pixi_dir = tempfile::tempdir().unwrap();
    let command_dispatcher = {
        let command_dispatcher = project
            .command_dispatcher_builder()
            .unwrap()
            .with_cache_dirs(CacheDirs::new(
                pixi_path::AbsPathBuf::new(temp_pixi_dir.path())
                    .expect("tempdir path should be absolute")
                    .into_assume_dir(),
            ));
        let command_dispatcher = if let Some(backend_override) = backend_override {
            command_dispatcher.with_backend_overrides(backend_override)
        } else {
            command_dispatcher
        };
        command_dispatcher.finish()
    };

    // Create UV context lazily for building dynamic metadata
    let uv_context: OnceCell<UvResolutionContext> = OnceCell::new();

    // Create build caches for sharing between satisfiability and resolution
    let build_caches: DashMap<BuildCacheKey, Arc<PypiEnvironmentBuildCache>> = DashMap::new();

    // Create static metadata cache for sharing across platforms
    let static_metadata_cache: DashMap<PathBuf, pypi_metadata::LocalPackageMetadata> =
        DashMap::new();

    let resolver = LockFileResolver::build(lock_file, project.root())
        .map_err(|err| LockfileUnsat::ResolverBuild(err.to_string()))?;

    // Verify individual environment satisfiability
    for env in project.environments() {
        let locked_env = lock_file
            .environment(env.name().as_str())
            .ok_or_else(|| LockfileUnsat::EnvironmentMissing(env.name().to_string()))?;
        verify_environment_satisfiability(&env, locked_env)
            .map_err(|e| LockfileUnsat::Environment(env.name().to_string(), e))?;

        for platform in env.platforms() {
            let ctx = VerifySatisfiabilityContext {
                environment: &env,
                command_dispatcher: command_dispatcher.clone(),
                platform,
                project_root: project.root(),
                uv_context: &uv_context,
                config: project.config(),
                project_env_vars: project.env_vars().clone(),
                build_caches: &build_caches,
                static_metadata_cache: &static_metadata_cache,
                resolver: &resolver,
            };
            let (verified_env, _locked_pypi) = verify_platform_satisfiability(&ctx, locked_env)
                .await
                .map_err(|e| match e {
                    CommandDispatcherError::Failed(e) => {
                        LockfileUnsat::PlatformUnsat(env.name().to_string(), platform, *e)
                    }
                    CommandDispatcherError::Cancelled => {
                        panic!("operation was cancelled which should never happen here")
                    }
                })?;

            individual_verified_envs.insert((env.name(), platform), verified_env);
        }
    }

    // Verify the solve group requirements
    for solve_group in project.solve_groups() {
        for platform in solve_group.platforms() {
            verify_solve_group_satisfiability(
                solve_group
                    .environments()
                    .filter_map(|env| individual_verified_envs.remove(&(env.name(), platform))),
            )
            .map_err(|e| {
                LockfileUnsat::SolveGroupUnsat(solve_group.name().to_string(), platform, e)
            })?;
        }
    }

    // Verify environments not part of a solve group
    for ((env_name, platform), verified_env) in individual_verified_envs.into_iter() {
        verify_solve_group_satisfiability([verified_env])
            .map_err(|e| match e {
                SolveGroupUnsat::CondaPackageShouldBePypi { name } => {
                    PlatformUnsat::CondaPackageShouldBePypi { name }
                }
            })
            .map_err(|e| LockfileUnsat::PlatformUnsat(env_name.to_string(), platform, e))?;
    }

    Ok(())
}

#[rstest]
#[tokio::test]
#[traced_test]
async fn test_good_satisfiability(
    #[files("../../tests/data/satisfiability/*/pixi.toml")] manifest_path: PathBuf,
) {
    // TODO: skip this test on windows
    // Until we can figure out how to handle unix file paths with pep508_rs url
    // parsing correctly
    if manifest_path
        .components()
        .contains(&Component::Normal(OsStr::new("absolute-paths")))
        && cfg!(windows)
    {
        return;
    }

    let project = Workspace::from_path(&manifest_path).unwrap();
    let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
    match verify_lockfile_satisfiability(
        &project,
        &lock_file,
        Some(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        )),
    )
    .await
    .into_diagnostic()
    {
        Ok(()) => {}
        Err(e) => panic!("{e:?}"),
    }
}

#[rstest]
#[tokio::test]
#[traced_test]
async fn q(#[files("../../examples/**/p*.toml")] manifest_path: PathBuf) {
    // If a pyproject.toml is present check for `tool.pixi` in the file to avoid
    // testing of non-pixi files
    if manifest_path.file_name().unwrap() == "pyproject.toml" {
        let manifest_str = fs_err::read_to_string(&manifest_path).unwrap();
        if !manifest_str.contains("tool.pixi.workspace") {
            return;
        }
    }

    // If a pixi.toml is present check for `workspace` in the file to avoid
    // testing of non-pixi workspace files
    if manifest_path.file_name().unwrap() == "pixi.toml" {
        let manifest_str = fs_err::read_to_string(&manifest_path).unwrap();
        if !manifest_str.contains("workspace") {
            return;
        }
    }

    let project = Workspace::from_path(&manifest_path).unwrap();
    let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
    match verify_lockfile_satisfiability(&project, &lock_file, None)
        .await
        .into_diagnostic()
    {
        Ok(()) => {}
        Err(e) => panic!("{e:?}"),
    }
}

#[rstest]
#[tokio::test]
#[traced_test]
async fn test_failing_satisfiability(
    #[files("../../tests/data/non-satisfiability/*/pixi.toml")] manifest_path: PathBuf,
) {
    let report_handler = NarratableReportHandler::new().with_cause_chain();

    let project = Workspace::from_path(&manifest_path).unwrap();
    let lock_file = LockFile::from_path(&project.lock_file_path()).unwrap();
    let err = verify_lockfile_satisfiability(
        &project,
        &lock_file,
        Some(BackendOverride::from_memory(
            PassthroughBackend::instantiator(),
        )),
    )
    .await
    .expect_err("expected failing satisfiability");

    let name = manifest_path
        .parent()
        .unwrap()
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap();

    let mut s = String::new();
    report_handler.render_report(&mut s, &err).unwrap();

    let mut settings = Settings::clone_current();
    settings.set_snapshot_suffix(name);
    settings.bind(|| {
        // run snapshot test here
        insta::assert_snapshot!(s);
    });
}

#[test]
fn test_version_specifiers_logic() {
    let version = Version::from_str("1.19").unwrap();
    let version_specifiers = VersionSpecifiers::from_str("<2.0, >=1.16").unwrap();
    assert!(version_specifiers.contains(&version));
    // VersionSpecifiers derefs into a list of specifiers
    assert_eq!(
        version_specifiers
            .iter()
            .position(|specifier| *specifier.operator() == Operator::LessThan),
        Some(1)
    );
}
