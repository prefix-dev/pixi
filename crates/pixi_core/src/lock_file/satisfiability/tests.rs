use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    path::Component,
    str::FromStr,
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
use pixi_install_pypi::{LockedPypiRecord, UnresolvedPypiRecord};
use pixi_manifest::FeaturesExt;
use pixi_manifest::pypi::pypi_options::NoBuild;
use pixi_pypi_spec::PixiPypiSource;
use pixi_record::LockFileResolver;
use pixi_uv_context::UvResolutionContext;
use pixi_uv_conversions::pep508_requirement_to_uv_requirement;
use rattler_conda_types::Platform;
use rattler_lock::{LockFile, PypiPackageData, UrlOrPath, Verbatim};
use rstest::rstest;
use thiserror::Error;
use tracing_test::traced_test;
use url::Url;
use uv_distribution_types::RequirementSource;

use super::{
    EnvironmentUnsat, PlatformUnsat, PypiNoBuildCheck, SolveGroupUnsat,
    VerifySatisfiabilityContext, pypi_metadata, pypi_satisfies_requirement,
    verify_environment_satisfiability, verify_platform_satisfiability,
    verify_solve_group_satisfiability,
};
use crate::{
    Workspace,
    lock_file::{
        outdated::{BuildCacheKey, PypiEnvironmentBuildCache},
        tests::{make_source_package_with, make_wheel_package_with},
    },
};

/// Lock a `PypiPackageData` into a `LockedPypiRecord` for testing.
/// Uses the package version for wheels, a dummy version for source packages.
fn lock_for_test(data: PypiPackageData) -> LockedPypiRecord {
    let version = data
        .version()
        .cloned()
        .unwrap_or_else(|| Version::from_str("42.23").unwrap());
    UnresolvedPypiRecord::from(data).lock(version)
}

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
fn test_pypi_git_check_with_rev() {
    // Mock locked data
    let locked_data = lock_for_test(make_wheel_package_with(
        "mypkg",
        "0.1.0",
        "git+https://github.com/mypkg@rev=29932f3915935d773dc8d52c292cadd81c81071d#29932f3915935d773dc8d52c292cadd81c81071d"
            .parse()
            .expect("failed to parse url"),
        None,
        None,
        vec![],
        None,
    ));
    let spec = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@2993").unwrap(),
    )
    .unwrap();
    let project_root = PathBuf::from_str("/").unwrap();
    // This will not satisfy because the rev length is different, even being
    // resolved to the same one
    pypi_satisfies_requirement(&spec, &locked_data, &project_root).unwrap_err();

    let locked_data = lock_for_test(make_wheel_package_with(
        "mypkg",
        "0.1.0",
        "git+https://github.com/mypkg.git?rev=29932f3915935d773dc8d52c292cadd81c81071d#29932f3915935d773dc8d52c292cadd81c81071d"
            .parse()
            .expect("failed to parse url"),
        None,
        None,
        vec![],
        None,
    ));
    let spec = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str(
            "mypkg @ git+https://github.com/mypkg.git@29932f3915935d773dc8d52c292cadd81c81071d",
        )
        .unwrap(),
    )
    .unwrap();
    let project_root = PathBuf::from_str("/").unwrap();
    // This will satisfy
    pypi_satisfies_requirement(&spec, &locked_data, &project_root).unwrap();
    let non_matching_spec = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg@defgd").unwrap(),
    )
    .unwrap();
    pypi_satisfies_requirement(&non_matching_spec, &locked_data, &project_root).unwrap_err();

    // Removing the rev from the Requirement should NOT satisfy when lock has
    // explicit Rev. This ensures that when a user removes an explicit ref
    // from the manifest, the lock file gets re-resolved.
    let spec_without_rev = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg").unwrap(),
    )
    .unwrap();
    pypi_satisfies_requirement(&spec_without_rev, &locked_data, &project_root).unwrap_err();

    // When lock has DefaultBranch (no explicit ref), removing rev from manifest
    // should satisfy
    // No ?rev= query param, only the fragment with commit hash
    let locked_data_default_branch = lock_for_test(make_wheel_package_with(
        "mypkg",
        "0.1.0",
        "git+https://github.com/mypkg.git#29932f3915935d773dc8d52c292cadd81c81071d"
            .parse()
            .expect("failed to parse url"),
        None,
        None,
        vec![],
        None,
    ));
    pypi_satisfies_requirement(
        &spec_without_rev,
        &locked_data_default_branch,
        &project_root,
    )
    .unwrap();
}

// Do not use unix paths on windows: The path gets normalized to something
// unix-y, and the lockfile keeps the "pretty" path the user filled in at
// all times. So on windows the test fails.
#[cfg(not(target_os = "windows"))]
#[test]
fn test_unix_absolute_path_handling() {
    let locked_data = lock_for_test(make_wheel_package_with(
        "mypkg",
        "0.1.0",
        Verbatim::new(UrlOrPath::Path("/home/username/mypkg.tar.gz".into())),
        None,
        None,
        vec![],
        None,
    ));

    let spec =
        pep508_rs::Requirement::from_str("mypkg @ file:///home/username/mypkg.tar.gz").unwrap();

    let spec = pep508_requirement_to_uv_requirement(spec).unwrap();

    pypi_satisfies_requirement(&spec, &locked_data, Path::new("")).unwrap();
}

#[test]
fn test_windows_absolute_path_handling() {
    let locked_data = lock_for_test(make_wheel_package_with(
        "mypkg",
        "0.1.0",
        Verbatim::new(UrlOrPath::Path("C:\\Users\\username\\mypkg.tar.gz".into())),
        None,
        None,
        vec![],
        None,
    ));

    let spec =
        pep508_rs::Requirement::from_str("mypkg @ file:///C:\\Users\\username\\mypkg.tar.gz")
            .unwrap();

    let spec = pep508_requirement_to_uv_requirement(spec).unwrap();

    pypi_satisfies_requirement(&spec, &locked_data, Path::new("")).unwrap();
}

// Validate uv documentation to avoid breaking change in pixi
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

// Regression test for https://github.com/prefix-dev/pixi/issues/5553
#[test]
fn pypi_editable_satisfied() {
    let pypi_no_build_check = PypiNoBuildCheck::new(Some(&NoBuild::All));

    pypi_no_build_check
        .check(
            &make_source_package_with(
                "sdist",
                UrlOrPath::from_str(".").expect("invalid path").into(),
                vec![],
                None,
            )
            .into(),
            Some(&PixiPypiSource::Path {
                path: PathBuf::from("").into(),
                editable: Some(true),
            }),
        )
        .expect("check must pass");
}

/// Test that `pypi_satisfies_requirement` works correctly when a pypi
/// package has no version (dynamic version from a source dependency).
/// Path-based requirements should still satisfy.
#[cfg(not(target_os = "windows"))]
#[test]
fn test_pypi_satisfies_path_requirement_without_version() {
    let locked_data = lock_for_test(make_source_package_with(
        "dynamic-dep",
        Verbatim::new(UrlOrPath::Path("/home/user/project/dynamic-dep".into())),
        vec![],
        None,
    ));

    let spec = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str("dynamic-dep @ file:///home/user/project/dynamic-dep")
            .unwrap(),
    )
    .unwrap();

    // A path-based source dependency without a version should still satisfy
    // a path-based requirement.
    pypi_satisfies_requirement(&spec, &locked_data, Path::new("")).unwrap();
}

/// Windows variant of the path-based dynamic version test.
#[cfg(target_os = "windows")]
#[test]
fn test_pypi_satisfies_path_requirement_without_version() {
    let locked_data = lock_for_test(make_source_package_with(
        "dynamic-dep",
        Verbatim::new(UrlOrPath::Path(
            "C:\\Users\\user\\project\\dynamic-dep".into(),
        )),
        vec![],
        None,
    ));

    let spec = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str(
            "dynamic-dep @ file:///C:\\Users\\user\\project\\dynamic-dep",
        )
        .unwrap(),
    )
    .unwrap();

    // A path-based source dependency without a version should still satisfy
    // a path-based requirement.
    pypi_satisfies_requirement(&spec, &locked_data, Path::new("")).unwrap();
}

/// Test that `pypi_satisfies_requirement` works with a git-based
/// requirement when the locked package has no version.
#[test]
fn test_pypi_satisfies_git_requirement_without_version() {
    let locked_data = lock_for_test(make_source_package_with(
        "mypkg",
        "git+https://github.com/mypkg.git#29932f3915935d773dc8d52c292cadd81c81071d"
            .parse()
            .expect("failed to parse url"),
        vec![],
        None,
    ));

    let spec = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str("mypkg @ git+https://github.com/mypkg").unwrap(),
    )
    .unwrap();

    // A git-based source dependency without a version should still satisfy.
    pypi_satisfies_requirement(&spec, &locked_data, Path::new("")).unwrap();
}

/// Regression test: removing a PyPI `index` from the manifest should
/// invalidate the lock-file when the locked package was resolved from that
/// index.
///
/// Verify that removing an explicit index from a PyPI requirement
/// invalidates the lock-file entry that was resolved from that index.
#[test]
fn test_pypi_index_removed_should_invalidate() {
    // Locked data: package was resolved from a custom index.
    let locked_data = lock_for_test(make_wheel_package_with(
        "my-dep",
        "1.0.0",
        "https://custom.example.com/simple/packages/my_dep-1.0.0-py3-none-any.whl"
            .parse()
            .expect("failed to parse url"),
        None,
        Some(Url::parse("https://custom.example.com/simple").unwrap()),
        vec![],
        None,
    ));

    // Requirement: no index specified (user removed the `index` field).
    let spec = pep508_requirement_to_uv_requirement(
        pep508_rs::Requirement::from_str("my-dep>=1.0").unwrap(),
    )
    .unwrap();

    let project_root = PathBuf::from_str("/").unwrap();

    let result = pypi_satisfies_requirement(&spec, &locked_data, &project_root);
    assert!(
        result.is_err(),
        "expected index removal to invalidate satisfiability, \
         but pypi_satisfies_requirement returned Ok(())"
    );
}

/// Helper to build a `uv_distribution_types::Requirement` with an explicit index.
fn registry_requirement_with_index(
    name: &str,
    specifier: &str,
    index_url: &str,
) -> uv_distribution_types::Requirement {
    use uv_normalize::PackageName as UvPackageName;
    use uv_pep440::VersionSpecifiers;

    let index =
        uv_distribution_types::IndexMetadata::from(uv_distribution_types::IndexUrl::from(
            uv_pep508::VerbatimUrl::from_url(Url::parse(index_url).unwrap().into()),
        ));
    uv_distribution_types::Requirement {
        name: UvPackageName::from_str(name).unwrap(),
        extras: vec![].into(),
        groups: vec![].into(),
        marker: uv_pep508::MarkerTree::TRUE,
        source: RequirementSource::Registry {
            specifier: VersionSpecifiers::from_str(specifier).unwrap(),
            index: Some(index),
            conflict: None,
        },
        origin: None,
    }
}

/// Verify that changing a PyPI index to a different non-default index
/// invalidates the lock-file.
#[test]
fn test_pypi_index_changed_should_invalidate() {
    let locked_data = lock_for_test(make_wheel_package_with(
        "my-dep",
        "1.0.0",
        "https://old-index.example.com/packages/my_dep-1.0.0-py3-none-any.whl"
            .parse()
            .expect("failed to parse url"),
        None,
        Some(Url::parse("https://old-index.example.com/simple").unwrap()),
        vec![],
        None,
    ));

    let spec = registry_requirement_with_index(
        "my-dep",
        ">=1.0",
        "https://new-index.example.com/simple",
    );

    let project_root = PathBuf::from_str("/").unwrap();
    let result = pypi_satisfies_requirement(&spec, &locked_data, &project_root);
    assert!(
        result.is_err(),
        "expected index change to invalidate satisfiability"
    );
}

/// Verify that a matching non-default index is considered satisfiable.
#[test]
fn test_pypi_index_matching_should_satisfy() {
    let index_url = "https://custom.example.com/simple";
    let locked_data = lock_for_test(make_wheel_package_with(
        "my-dep",
        "1.0.0",
        "https://custom.example.com/packages/my_dep-1.0.0-py3-none-any.whl"
            .parse()
            .expect("failed to parse url"),
        None,
        Some(Url::parse(index_url).unwrap()),
        vec![],
        None,
    ));

    let spec = registry_requirement_with_index("my-dep", ">=1.0", index_url);

    let project_root = PathBuf::from_str("/").unwrap();
    let result = pypi_satisfies_requirement(&spec, &locked_data, &project_root);
    assert!(
        result.is_ok(),
        "expected matching index to satisfy, got: {:?}",
        result.unwrap_err()
    );
}

/// Verify that adding an index to a requirement that was locked with the
/// default index invalidates the lock-file.
#[test]
fn test_pypi_index_added_should_invalidate() {
    let locked_data = lock_for_test(make_wheel_package_with(
        "my-dep",
        "1.0.0",
        "https://pypi.org/packages/my_dep-1.0.0-py3-none-any.whl"
            .parse()
            .expect("failed to parse url"),
        None,
        Some(Url::parse("https://pypi.org/simple").unwrap()),
        vec![],
        None,
    ));

    let spec =
        registry_requirement_with_index("my-dep", ">=1.0", "https://custom.example.com/simple");

    let project_root = PathBuf::from_str("/").unwrap();
    let result = pypi_satisfies_requirement(&spec, &locked_data, &project_root);
    assert!(
        result.is_err(),
        "expected adding an index to invalidate satisfiability"
    );
}

/// V6 lockfiles don't store per-package PyPI index URLs, so
/// `index_url` is `None` after parsing. When the manifest specifies a
/// per-package `index`, the satisfiability check must not treat the
/// missing locked index as a mismatch — it is simply absent from the
/// older format.
///
/// This is a regression test for a bug observed in crater runs where
/// `pixi install --all` upgraded v6 lockfiles to v7.
#[test]
fn test_v6_missing_index_url_should_not_invalidate() {
    let index_url = "https://custom.example.com/simple";

    // Simulate a v6 locked package: resolved from a custom index, but
    // index_url is None because v6 doesn't store it.
    let locked_data = lock_for_test(make_wheel_package_with(
        "my-dep",
        "1.0.0",
        "https://custom.example.com/packages/my_dep-1.0.0-py3-none-any.whl"
            .parse()
            .expect("failed to parse url"),
        None,
        None, // v6: no per-package index_url
        vec![],
        None,
    ));

    let spec = registry_requirement_with_index("my-dep", ">=1.0", index_url);

    let project_root = PathBuf::from_str("/").unwrap();
    let result = pypi_satisfies_requirement(&spec, &locked_data, &project_root);
    assert!(
        result.is_ok(),
        "v6 lockfile with missing index_url should still satisfy a \
         requirement with an explicit index, got: {:?}",
        result.unwrap_err()
    );
}

/// Helpers and unit tests for the build/host verification path
/// added on top of `verify_platform_satisfiability` (the entrypoint
/// that replaces the old `SourceRecordRequiresRebuild` stopgap).
mod backend_verification {
    use super::super::{
        BuildOrHostEnv, build_full_source_record_from_output, variants_equivalent,
        verify_locked_against_backend_specs,
    };
    use pixi_build_types::{
        BinaryPackageSpec, NamedSpec, PackageSpec, PinCompatibleSpec, SourcePackageName,
        VariantValue,
        procedures::conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputIgnoreRunExports,
            CondaOutputMetadata, CondaOutputRunExports,
        },
    };
    use pixi_record::{
        PartialSourceRecordData, PinnedPathSpec, PinnedSourceSpec, SourceRecordData,
        UnresolvedPixiRecord, UnresolvedSourceRecord,
    };
    use pixi_spec::{SourceAnchor, SourceLocationSpec};
    use rattler_conda_types::{
        ChannelConfig, NoArchType, PackageName, PackageRecord, Platform, RepoDataRecord,
        VersionSpec, VersionWithSource, package::DistArchiveIdentifier,
    };
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        str::FromStr,
        sync::{Arc, LazyLock},
    };
    use url::Url;

    static CHANNEL_CONFIG: LazyLock<ChannelConfig> =
        LazyLock::new(|| ChannelConfig::default_with_root_dir(PathBuf::from("/workspace")));

    fn make_binary_record(name: &str, version: &str) -> RepoDataRecord {
        let pkg_name = PackageName::from_str(name).expect("valid name");
        let mut pr = PackageRecord::new(
            pkg_name,
            VersionWithSource::from_str(version).expect("valid version"),
            "h0".into(),
        );
        pr.subdir = "linux-64".into();
        let file_name = format!("{name}-{version}-h0.conda");
        RepoDataRecord {
            package_record: pr,
            identifier: DistArchiveIdentifier::from_str(&file_name)
                .expect("valid dist archive identifier"),
            url: Url::parse(&format!(
                "https://example.com/conda-forge/linux-64/{file_name}"
            ))
            .expect("valid url"),
            channel: Some("https://example.com/conda-forge".to_string()),
        }
    }

    fn binary_dep(name: &str, spec_str: &str) -> NamedSpec<PackageSpec> {
        let spec = if spec_str.is_empty() {
            BinaryPackageSpec::default()
        } else {
            BinaryPackageSpec {
                version: Some(
                    VersionSpec::from_str(
                        spec_str,
                        rattler_conda_types::ParseStrictness::Lenient,
                    )
                    .expect("valid spec"),
                ),
                ..Default::default()
            }
        };
        NamedSpec {
            name: SourcePackageName::from(PackageName::from_str(name).expect("valid name")),
            spec: PackageSpec::Binary(spec),
        }
    }

    fn pin_compatible_dep(name: &str) -> NamedSpec<PackageSpec> {
        pin_compatible_dep_with(
            name,
            PinCompatibleSpec {
                lower_bound: None,
                upper_bound: None,
                exact: false,
                build: None,
            },
        )
    }

    fn pin_compatible_dep_with(name: &str, spec: PinCompatibleSpec) -> NamedSpec<PackageSpec> {
        NamedSpec {
            name: SourcePackageName::from(PackageName::from_str(name).expect("valid name")),
            spec: PackageSpec::PinCompatible(spec),
        }
    }

    fn make_partial_source_record(
        name: &str,
        manifest_path: &str,
        build_packages: Vec<UnresolvedPixiRecord>,
        host_packages: Vec<UnresolvedPixiRecord>,
    ) -> UnresolvedSourceRecord {
        UnresolvedSourceRecord {
            data: SourceRecordData::Partial(PartialSourceRecordData {
                name: PackageName::from_str(name).unwrap(),
                depends: Vec::new(),
                constrains: Vec::new(),
                experimental_extra_depends: Default::default(),
                flags: Default::default(),
                purls: None,
                run_exports: None,
                sources: Default::default(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: manifest_path.into(),
            }),
            build_source: None,
            variants: Default::default(),
            identifier_hash: None,
            build_packages,
            host_packages,
        }
    }

    fn make_conda_output(name: &str, build_deps: Vec<NamedSpec<PackageSpec>>) -> CondaOutput {
        CondaOutput {
            metadata: CondaOutputMetadata {
                name: PackageName::from_str(name).unwrap(),
                version: "1.0.0"
                    .parse::<rattler_conda_types::Version>()
                    .unwrap()
                    .into(),
                build: "h0_0".to_string(),
                build_number: 0,
                subdir: Platform::Linux64,
                license: None,
                license_family: None,
                noarch: NoArchType::none(),
                purls: None,
                python_site_packages_path: None,
                variant: BTreeMap::new(),
            },
            build_dependencies: Some(CondaOutputDependencies {
                depends: build_deps,
                constraints: Vec::new(),
            }),
            host_dependencies: None,
            run_dependencies: CondaOutputDependencies {
                depends: Vec::new(),
                constraints: Vec::new(),
            },
            ignore_run_exports: CondaOutputIgnoreRunExports::default(),
            run_exports: CondaOutputRunExports::default(),
            input_globs: None,
        }
    }

    #[test]
    fn variants_equivalent_ignores_target_platform() {
        // Locked record with no variants vs backend output that
        // injected `target_platform=linux-64`: they should still
        // count as equivalent so older lock files (which omit the
        // synthetic key) keep matching.
        let locked = BTreeMap::new();
        let mut backend = BTreeMap::new();
        backend.insert(
            "target_platform".to_string(),
            VariantValue::String("linux-64".to_string()),
        );
        assert!(variants_equivalent(&locked, &backend));
    }

    #[test]
    fn variants_equivalent_real_keys_must_match() {
        let mut locked = BTreeMap::new();
        locked.insert(
            "python".to_string(),
            pixi_record::VariantValue::String("3.11".to_string()),
        );
        let mut backend = BTreeMap::new();
        backend.insert(
            "python".to_string(),
            VariantValue::String("3.10".to_string()),
        );
        assert!(!variants_equivalent(&locked, &backend));
    }

    #[test]
    fn locked_build_satisfies_backend_spec_passes() {
        // Backend declares `numpy >=1`; locked build_packages
        // contains numpy 1.5. Verification should pass.
        let locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(Arc::new(
            make_binary_record("numpy", "1.5"),
        ))];
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("numpy", ">=1")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let result = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        );
        assert!(result.is_ok(), "verification should pass: {result:?}");
    }

    #[test]
    fn locked_build_does_not_satisfy_backend_spec_fails() {
        // Backend declares `numpy >=2`; locked has numpy 1.5. Must
        // surface `SourceBuildHostUnsat` so the caller knows which
        // spec drifted.
        let locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(Arc::new(
            make_binary_record("numpy", "1.5"),
        ))];
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("numpy", ">=2")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let err = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        )
        .expect_err("locked numpy=1.5 must not satisfy >=2");
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    /// Regression: an early version of `LockedConda::satisfies_binary`
    /// matched a `NamelessMatchSpec` against a `RepoDataRecord`
    /// without checking the package name first. With a wildcard
    /// spec like `bar *`, every locked binary record (including
    /// `numpy 1.5`) was reported as satisfying it. The check now
    /// requires the record's name to match the spec's caller-
    /// supplied name.
    #[test]
    fn wrong_name_record_does_not_satisfy_binary_spec() {
        // Backend wants `bar *`. Locked has only `foo 1.5`.
        // A name-blind matcher would falsely accept `foo 1.5`.
        let locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(Arc::new(
            make_binary_record("foo", "1.5"),
        ))];
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("bar", "")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let err = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        )
        .expect_err("name mismatch must surface as unsat");
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    #[test]
    fn missing_required_record_in_locked_build_fails() {
        // Backend wants `cmake` in build env; locked build is
        // empty. Must report `SourceBuildHostUnsat` rather than
        // silently passing.
        let locked: Vec<UnresolvedPixiRecord> = Vec::new();
        let deps = CondaOutputDependencies {
            depends: vec![binary_dep("cmake", "")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));
        let err = verify_locked_against_backend_specs(
            &deps,
            &locked,
            &[],
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Build,
        )
        .expect_err("missing record must surface as unsat");
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    #[test]
    fn build_full_source_record_preserves_locked_depends_and_pin() {
        // Locked partial record with non-trivial depends. The
        // backend output reports a fresh version/build but no
        // run_deps. The synthesized full record must keep the
        // locked depends (which carries previously-resolved
        // run-exports) and the locked manifest pin verbatim.
        let mut partial =
            make_partial_source_record("mypkg", "./mypkg", Vec::new(), Vec::new());
        // Hand-set locked depends so the assertion has something
        // distinctive to compare.
        if let SourceRecordData::Partial(p) = &mut partial.data {
            p.depends = vec!["numpy >=1".to_string(), "openssl 3.0.*".to_string()];
        }

        let output = make_conda_output("mypkg", Vec::new());
        let full = build_full_source_record_from_output(&partial, &output);
        assert_eq!(
            full.data.package_record.depends,
            vec!["numpy >=1".to_string(), "openssl 3.0.*".to_string()],
            "locked depends must survive into the synthesized full record"
        );
        assert_eq!(full.manifest_source, partial.manifest_source);
    }

    /// `pin_compatible(foo)` in *host* dependencies pins against
    /// the version of `foo` resolved in the *build* environment.
    /// If the locked build env has no `foo`, no re-solve can
    /// succeed (the resolver would fail with
    /// `PinCompatibleError::PackageNotFound`), so the lock must
    /// be rejected even when the host env happens to carry a
    /// `foo` from another dep.
    #[test]
    fn pin_compatible_host_dep_rejects_when_build_lacks_package() {
        let host_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];
        let build_locked: Vec<UnresolvedPixiRecord> = Vec::new();

        let host_deps = CondaOutputDependencies {
            depends: vec![pin_compatible_dep("numpy")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));

        let err = verify_locked_against_backend_specs(
            &host_deps,
            &host_locked,
            &build_locked,
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Host,
        )
        .expect_err(
            "pin_compatible(numpy) must resolve against the (empty) build env, \
             not the host env that happens to contain numpy",
        );
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    /// Happy path: locked build env has `numpy 1.5`, locked host
    /// env also has `numpy 1.5`, host dep is `pin_compatible(numpy)`
    /// with no bounds (resolves to `*`). Verification passes.
    #[test]
    fn pin_compatible_host_dep_satisfied() {
        let host_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];
        let build_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];

        let host_deps = CondaOutputDependencies {
            depends: vec![pin_compatible_dep("numpy")],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));

        let result = verify_locked_against_backend_specs(
            &host_deps,
            &host_locked,
            &build_locked,
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Host,
        );
        assert!(result.is_ok(), "verification should pass: {result:?}");
    }

    /// Resolution-then-verification: build env has `numpy 2.0`, the
    /// pin is `exact=true`, and host env still carries `numpy 1.5`
    /// from before the user bumped the build env. The resolved
    /// spec is `numpy ==2.0`, which the locked host record does
    /// not satisfy.
    #[test]
    fn pin_compatible_host_dep_rejects_version_drift() {
        use pixi_build_types::PinCompatibleSpec;

        let host_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "1.5")),
        )];
        let build_locked: Vec<UnresolvedPixiRecord> = vec![UnresolvedPixiRecord::Binary(
            Arc::new(make_binary_record("numpy", "2.0")),
        )];

        let host_deps = CondaOutputDependencies {
            depends: vec![pin_compatible_dep_with(
                "numpy",
                PinCompatibleSpec {
                    lower_bound: None,
                    upper_bound: None,
                    exact: true,
                    build: None,
                },
            )],
            constraints: Vec::new(),
        };
        let anchor = SourceAnchor::from(SourceLocationSpec::from(PinnedSourceSpec::Path(
            PinnedPathSpec {
                path: "./pkg".into(),
            },
        )));

        let err = verify_locked_against_backend_specs(
            &host_deps,
            &host_locked,
            &build_locked,
            &CHANNEL_CONFIG,
            &anchor,
            &PackageName::from_str("pkg").unwrap(),
            BuildOrHostEnv::Host,
        )
        .expect_err(
            "host's locked numpy 1.5 cannot satisfy pin_compatible(numpy, exact) \
             against build's numpy 2.0",
        );
        assert!(
            matches!(
                *err,
                super::super::PlatformUnsat::SourceBuildHostUnsat { .. }
            ),
            "expected SourceBuildHostUnsat, got: {err}"
        );
    }

    // -- Unit tests for run-dependency / run-constraint drift -----------

    use super::super::{
        SourceRunDepKind, diff_dep_sequences, verify_locked_run_deps_against_backend,
    };
    use pixi_record::FullSourceRecordData;

    #[test]
    fn diff_sequences_passes_when_equal() {
        let result = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["a >=1".to_string(), "b ==2".to_string()],
        );
        assert!(
            result.is_ok(),
            "identical sequences should not drift: {result:?}"
        );
    }

    #[test]
    fn diff_sequences_ignores_reorder() {
        // Same multiset, different order. Order is not semantically
        // meaningful; only the symmetric multiset difference matters.
        let result = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["b ==2".to_string(), "a >=1".to_string()],
        );
        assert!(
            result.is_ok(),
            "permutations must not surface as drift: {result:?}"
        );
    }

    #[test]
    fn diff_sequences_reports_only_addition() {
        let diff = diff_dep_sequences(
            &["a >=1".to_string()],
            &["a >=1".to_string(), "b ==2".to_string()],
        )
        .expect_err("expected drift");
        assert_eq!(diff.added, vec!["b ==2".to_string()]);
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_sequences_reports_only_removal() {
        let diff = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["a >=1".to_string()],
        )
        .expect_err("expected drift");
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["b ==2".to_string()]);
    }

    #[test]
    fn diff_sequences_reports_both_directions() {
        let diff = diff_dep_sequences(
            &["a >=1".to_string(), "b ==2".to_string()],
            &["a >=1".to_string(), "c <=3".to_string()],
        )
        .expect_err("expected drift");
        assert_eq!(diff.added, vec!["c <=3".to_string()]);
        assert_eq!(diff.removed, vec!["b ==2".to_string()]);
    }

    #[test]
    fn diff_sequences_treats_duplicates_as_distinct() {
        // Locked carries the same spec twice but the expected set
        // only carries it once; the extra copy must surface as a
        // removal.
        let diff = diff_dep_sequences(
            &["a >=1".to_string(), "a >=1".to_string()],
            &["a >=1".to_string()],
        )
        .expect_err("expected drift");
        assert!(diff.added.is_empty());
        assert_eq!(diff.removed, vec!["a >=1".to_string()]);
    }

    /// Build a Full source record with the supplied `depends` and
    /// `constrains` strings. Build/host packages are empty, which
    /// is enough for the constrains-only test cases below.
    fn make_full_source_record(
        name: &str,
        depends: Vec<String>,
        constrains: Vec<String>,
    ) -> UnresolvedSourceRecord {
        let pkg_name = PackageName::from_str(name).unwrap();
        let mut pr = PackageRecord::new(
            pkg_name.clone(),
            "1.0.0"
                .parse::<rattler_conda_types::VersionWithSource>()
                .unwrap(),
            "h0_0".into(),
        );
        pr.subdir = "linux-64".into();
        pr.depends = depends;
        pr.constrains = constrains;
        UnresolvedSourceRecord {
            data: SourceRecordData::Full(FullSourceRecordData {
                package_record: pr,
                sources: Default::default(),
            }),
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: "./pkg".into(),
            }),
            build_source: None,
            variants: Default::default(),
            identifier_hash: None,
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        }
    }

    /// Helper to build a `CondaOutput` whose `run_dependencies` has
    /// the given `depends` and `constraints`. Other fields default
    /// to empty.
    fn make_conda_output_with_run_deps(
        name: &str,
        depends: Vec<NamedSpec<PackageSpec>>,
        constraints: Vec<NamedSpec<pixi_build_types::ConstraintSpec>>,
    ) -> CondaOutput {
        let mut output = make_conda_output(name, Vec::new());
        output.build_dependencies = None;
        output.run_dependencies = CondaOutputDependencies {
            depends,
            constraints,
        };
        output
    }

    fn binary_constraint(
        name: &str,
        spec_str: &str,
    ) -> NamedSpec<pixi_build_types::ConstraintSpec> {
        NamedSpec {
            name: SourcePackageName::from(PackageName::from_str(name).unwrap()),
            spec: pixi_build_types::ConstraintSpec::Binary(BinaryPackageSpec {
                version: Some(
                    VersionSpec::from_str(
                        spec_str,
                        rattler_conda_types::ParseStrictness::Lenient,
                    )
                    .unwrap(),
                ),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn verify_locked_run_deps_passes_when_match() {
        // Backend declares run_deps `numpy >=1` and constrains
        // `openssl ==3.0`; locked record has the same. No drift.
        let record = make_full_source_record(
            "pkg",
            vec!["numpy >=1".to_string()],
            vec!["openssl ==3.0".to_string()],
        );
        let output = make_conda_output_with_run_deps(
            "pkg",
            vec![binary_dep("numpy", ">=1")],
            vec![binary_constraint("openssl", "==3.0")],
        );

        let result = verify_locked_run_deps_against_backend(&record, &output, &CHANNEL_CONFIG);
        assert!(result.is_ok(), "expected no drift: {result:?}");
    }

    #[test]
    fn verify_locked_run_deps_detects_constrain_addition() {
        // Backend declares a new constrain `bar <2` that the locked
        // record does not carry. Drift surfaces with `kind =
        // RunConstrains` and `added = ["bar <2"]`.
        let record = make_full_source_record("pkg", Vec::new(), Vec::new());
        let output = make_conda_output_with_run_deps(
            "pkg",
            Vec::new(),
            vec![binary_constraint("bar", "<2")],
        );

        let err = verify_locked_run_deps_against_backend(&record, &output, &CHANNEL_CONFIG)
            .expect_err("backend declared a new constraint, locked has none");
        match *err {
            super::super::PlatformUnsat::SourceRunDependenciesChanged {
                kind: SourceRunDepKind::RunConstrains,
                added,
                removed,
                ..
            } => {
                assert_eq!(added, vec!["bar <2".to_string()]);
                assert!(removed.is_empty());
            }
            other => panic!("expected RunConstrains drift, got: {other}"),
        }
    }

    #[test]
    fn verify_locked_run_deps_detects_constrain_removal() {
        // Locked record carries a constrain that the backend no
        // longer declares. Drift surfaces with `removed`.
        let record = make_full_source_record("pkg", Vec::new(), vec!["bar <2".to_string()]);
        let output = make_conda_output_with_run_deps("pkg", Vec::new(), Vec::new());

        let err = verify_locked_run_deps_against_backend(&record, &output, &CHANNEL_CONFIG)
            .expect_err("backend dropped a constraint that's still locked");
        match *err {
            super::super::PlatformUnsat::SourceRunDependenciesChanged {
                kind: SourceRunDepKind::RunConstrains,
                added,
                removed,
                ..
            } => {
                assert!(added.is_empty());
                assert_eq!(removed, vec!["bar <2".to_string()]);
            }
            other => panic!("expected RunConstrains drift, got: {other}"),
        }
    }
}
