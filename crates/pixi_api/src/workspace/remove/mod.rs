use std::path::Path;

use indexmap::IndexMap;
use miette::Diagnostic;
use pep508_rs::Requirement;
use pixi_core::{
    InstallFilter, UpdateLockFileOptions,
    environment::{LockFileUsage, get_update_lock_file_and_prefix},
    lock_file::{ReinstallPackages, UpdateMode},
    workspace::{PypiDeps, WorkspaceMut},
};
use pixi_manifest::{
    DependencyError, FeatureName, FeaturesExt, LoadManifestsError, RemoveDependencyError, SpecType,
    TargetSelector, WorkspaceManifest,
};
use pixi_pypi_spec::PypiPackageName;
use rattler_conda_types::{MatchSpec, PackageName};
use thiserror::Error;

use crate::workspace::DependencyOptions;

#[derive(Debug, Error, Diagnostic)]
pub enum RemoveError {
    #[error("dependency `{name}` was not found")]
    NotFound { name: String },

    #[error(
        "dependency `{name}` exists in multiple locations: {}",
        .locations.join(", ")
    )]
    Ambiguous {
        name: String,
        locations: Vec<String>,
    },

    #[error("`{spec}` is not a valid Conda or PyPI dependency")]
    InvalidSpec { spec: String },

    #[error(
        "Cannot remove Python while PyPI dependencies exist. Please remove these PyPI dependencies first: {}",
        .pypi_deps.join(", ")
    )]
    PythonHasPypiDependencies { pypi_deps: Vec<String> },

    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    /// `NoDependency` is hoisted to [`Self::NotFound`] by `From<RemoveDependencyError>`.
    #[error(transparent)]
    #[diagnostic(transparent)]
    Manifest(RemoveDependencyError),

    #[error("failed to save the manifest")]
    Save(#[source] std::io::Error),

    #[error(transparent)]
    #[diagnostic(transparent)]
    LockFileUpdate(Box<dyn Diagnostic + Send + Sync + 'static>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DependencyLocation {
    Conda {
        name: PackageName,
        spec_type: SpecType,
        feature: FeatureName,
        target: Option<TargetSelector>,
    },
    Pypi {
        name: PypiPackageName,
        feature: FeatureName,
        target: Option<TargetSelector>,
    },
}

impl DependencyLocation {
    fn describe(&self) -> String {
        let (table, feature, target) = match self {
            DependencyLocation::Conda {
                spec_type,
                feature,
                target,
                ..
            } => (spec_type.name(), feature, target),
            DependencyLocation::Pypi {
                feature, target, ..
            } => ("pypi-dependencies", feature, target),
        };

        let feature = if feature.is_default() {
            "the default feature".to_string()
        } else {
            format!("feature `{feature}`")
        };
        match target {
            Some(target) => format!("{table} for target `{target}` in {feature}"),
            None => format!("{table} in {feature}"),
        }
    }
}

struct RequestedDependency {
    name: String,
    conda: Option<PackageName>,
    pypi: Option<PypiPackageName>,
}

fn parse_requested_dependency(
    spec: &str,
    workspace_root: &Path,
) -> Result<RequestedDependency, RemoveError> {
    let conda = MatchSpec::from_str(
        spec,
        rattler_conda_types::ParseMatchSpecOptions::lenient()
            .with_repodata_revision(rattler_conda_types::RepodataRevision::V3),
    )
    .ok()
    .and_then(|spec| spec.name.as_exact().cloned());
    let pypi = Requirement::<pep508_rs::VerbatimUrl>::parse(spec, workspace_root)
        .ok()
        .map(|requirement| PypiPackageName::from_normalized(requirement.name));

    if conda.is_none() && pypi.is_none() {
        return Err(RemoveError::InvalidSpec {
            spec: spec.to_string(),
        });
    }

    let name = conda
        .as_ref()
        .map(|name| name.as_source().to_string())
        .or_else(|| pypi.as_ref().map(|name| name.as_source().to_string()))
        .expect("at least one dependency parser succeeded");
    Ok(RequestedDependency { name, conda, pypi })
}

fn find_dependency_locations(
    manifest: &WorkspaceManifest,
    requested: &RequestedDependency,
) -> Vec<DependencyLocation> {
    let mut locations = Vec::new();
    for (feature_name, feature) in &manifest.features {
        for (target, selector) in feature.targets.iter() {
            if let Some(requested_name) = &requested.conda {
                for spec_type in SpecType::all() {
                    if let Some(name) = target.dependencies(spec_type).and_then(|dependencies| {
                        dependencies.names().find(|name| *name == requested_name)
                    }) {
                        locations.push(DependencyLocation::Conda {
                            name: name.clone(),
                            spec_type,
                            feature: feature_name.clone(),
                            target: selector.cloned(),
                        });
                    }
                }
            }
            if let Some(requested_name) = &requested.pypi
                && let Some(name) = target.pypi_dependencies.as_ref().and_then(|dependencies| {
                    dependencies.names().find(|name| *name == requested_name)
                })
            {
                locations.push(DependencyLocation::Pypi {
                    name: name.clone(),
                    feature: feature_name.clone(),
                    target: selector.cloned(),
                });
            }
        }
    }
    locations
}

impl From<RemoveDependencyError> for RemoveError {
    fn from(value: RemoveDependencyError) -> Self {
        match value {
            RemoveDependencyError::Dependency(DependencyError::NoDependency(name)) => {
                RemoveError::NotFound { name }
            }
            other => RemoveError::Manifest(other),
        }
    }
}

pub async fn remove_conda_deps(
    mut workspace: WorkspaceMut,
    specs: IndexMap<PackageName, MatchSpec>,
    spec_type: SpecType,
    options: DependencyOptions,
) -> Result<(), RemoveError> {
    // Prevent removing Python if PyPI dependencies exist
    for name in specs.keys() {
        if name.as_source() == "python" {
            let pypi_deps = workspace
                .workspace()
                .default_environment()
                .pypi_dependencies(None);
            if !pypi_deps.is_empty() {
                return Err(RemoveError::PythonHasPypiDependencies {
                    pypi_deps: pypi_deps
                        .iter()
                        .map(|(name, _)| name.as_source().to_string())
                        .collect(),
                });
            }
        }
    }

    for name in specs.keys() {
        workspace.manifest().remove_dependency(
            name,
            spec_type,
            &options.platforms,
            &options.feature,
        )?;
    }
    save_and_update(workspace, options).await
}

pub async fn remove_pypi_deps(
    mut workspace: WorkspaceMut,
    pypi_deps: PypiDeps,
    options: DependencyOptions,
) -> Result<(), RemoveError> {
    for name in pypi_deps.keys() {
        workspace
            .manifest()
            .remove_pypi_dependency(name, &options.platforms, &options.feature)?;
    }

    save_and_update(workspace, options).await
}

/// Removes unqualified dependency names by resolving each one across every
/// dependency table, feature, and target in the manifest.
pub async fn remove_deps_unqualified(
    mut workspace: WorkspaceMut,
    specs: Vec<String>,
    options: DependencyOptions,
) -> Result<(), RemoveError> {
    let requested = specs
        .iter()
        .map(|spec| parse_requested_dependency(spec, workspace.workspace().root()))
        .collect::<Result<Vec<_>, _>>()?;

    // Resolve every input before mutating anything so ambiguity or a missing
    // package cannot leave a partially edited manifest.
    let mut resolved = Vec::new();
    for requested in requested {
        let locations =
            find_dependency_locations(&workspace.workspace().workspace.value, &requested);
        match locations.as_slice() {
            [] => {
                return Err(RemoveError::NotFound {
                    name: requested.name,
                });
            }
            [location] => {
                if !resolved.contains(location) {
                    resolved.push(location.clone());
                }
            }
            locations => {
                return Err(RemoveError::Ambiguous {
                    name: requested.name,
                    locations: locations.iter().map(DependencyLocation::describe).collect(),
                });
            }
        }
    }

    if resolved.iter().any(|location| {
        matches!(
            location,
            DependencyLocation::Conda { name, .. } if name.as_source() == "python"
        )
    }) {
        let pypi_deps = workspace
            .workspace()
            .default_environment()
            .pypi_dependencies(None);
        if !pypi_deps.is_empty() {
            return Err(RemoveError::PythonHasPypiDependencies {
                pypi_deps: pypi_deps
                    .iter()
                    .map(|(name, _)| name.as_source().to_string())
                    .collect(),
            });
        }
    }

    {
        let mut manifest = workspace.manifest();
        for location in resolved {
            match location {
                DependencyLocation::Conda {
                    name,
                    spec_type,
                    feature,
                    target,
                } => manifest.remove_dependency_from_target(
                    &name,
                    spec_type,
                    target.as_ref(),
                    &feature,
                )?,
                DependencyLocation::Pypi {
                    name,
                    feature,
                    target,
                } => {
                    manifest.remove_pypi_dependency_from_target(&name, target.as_ref(), &feature)?
                }
            }
        }
    }

    save_and_update(workspace, options).await
}

async fn save_and_update(
    workspace: WorkspaceMut,
    options: DependencyOptions,
) -> Result<(), RemoveError> {
    let workspace = workspace.save().await.map_err(RemoveError::Save)?;

    // TODO: update all environments touched by this feature defined.
    // updating prefix after removing from toml
    if options.lock_file_usage == LockFileUsage::Update {
        get_update_lock_file_and_prefix(
            &workspace.default_environment(),
            None,
            UpdateMode::Revalidate,
            UpdateLockFileOptions {
                lock_file_usage: options.lock_file_usage,
                no_install: options.no_install,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
                ..Default::default()
            },
            ReinstallPackages::default(),
            &InstallFilter::default(),
        )
        .await
        .map_err(|e| RemoveError::LockFileUpdate(e.into()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use pixi_core::{Workspace, environment::LockFileUsage};
    use pixi_manifest::FeatureName;
    use pixi_test_utils::format_diagnostic;
    use rattler_conda_types::{MatchSpec, ParseMatchSpecOptions, RepodataRevision};

    use super::*;

    fn workspace_from(toml: &str) -> Workspace {
        // The workspace re-reads its manifest from disk during `modify()`, so
        // we write the TOML to a temp dir and load from that path. The dir is
        // intentionally leaked for the duration of the test.
        let tmp = tempfile::TempDir::new().unwrap().keep();
        let path = tmp.join("pixi.toml");
        fs_err::write(&path, toml).unwrap();
        Workspace::from_path(&path).expect("failed to load workspace")
    }

    fn workspace_from_pyproject(toml: &str) -> Workspace {
        let tmp = tempfile::TempDir::new().unwrap().keep();
        let path = tmp.join("pyproject.toml");
        fs_err::write(&path, toml).unwrap();
        Workspace::from_path(&path).expect("failed to load workspace")
    }

    fn options() -> DependencyOptions {
        DependencyOptions {
            feature: FeatureName::DEFAULT,
            platforms: vec![],
            no_install: true,
            lock_file_usage: LockFileUsage::Frozen,
        }
    }

    fn conda_spec(name: &str) -> (PackageName, MatchSpec) {
        let spec = MatchSpec::from_str(
            name,
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
        )
        .unwrap();
        (spec.name.as_exact().unwrap().clone(), spec)
    }

    /// `pixi remove python` while pypi dependencies still reference it should
    /// fail the python guard before touching the manifest.
    #[tokio::test]
    async fn python_guard_triggers_when_pypi_deps_present() {
        let workspace = workspace_from(
            r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]

[dependencies]
python = "*"

[pypi-dependencies]
requests = "*"
"#,
        );
        let (name, spec) = conda_spec("python");
        let mut specs = IndexMap::new();
        specs.insert(name, spec);

        let err = remove_conda_deps(workspace.modify().unwrap(), specs, SpecType::Run, options())
            .await
            .unwrap_err();

        insta::assert_snapshot!(
            format_diagnostic(&err),
            @"  × Cannot remove Python while PyPI dependencies exist. Please remove these PyPI dependencies first: requests"
        );
    }

    /// `pixi remove fizzbuzz` against a workspace that doesn't list fizzbuzz
    /// anywhere should land in the typed `NotFound` arm.
    #[tokio::test]
    async fn missing_dep_triggers_not_found() {
        let workspace = workspace_from(
            r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]

[dependencies]
ruff = "*"
"#,
        );
        let (name, spec) = conda_spec("fizzbuzz");
        let mut specs = IndexMap::new();
        specs.insert(name, spec);

        let err = remove_conda_deps(workspace.modify().unwrap(), specs, SpecType::Run, options())
            .await
            .unwrap_err();

        insta::assert_snapshot!(
            format_diagnostic(&err),
            @"  × dependency `fizzbuzz` was not found"
        );
        assert!(matches!(err, RemoveError::NotFound { name } if name == "fizzbuzz"));
    }

    #[tokio::test]
    async fn removes_unambiguous_dependencies_across_all_locations() {
        let workspace = workspace_from(
            r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]

[dependencies]
numpy = "*"

[pypi-dependencies]
black = "*"

[feature.test.dependencies]
pytest = "*"

[target.linux.dependencies]
bla = "*"
"#,
        );
        let manifest_path = workspace.workspace.provenance.path.clone();

        remove_deps_unqualified(
            workspace.modify().unwrap(),
            ["numpy", "black", "pytest", "bla"]
                .map(str::to_string)
                .to_vec(),
            options(),
        )
        .await
        .unwrap();

        let manifest = fs_err::read_to_string(manifest_path).unwrap();
        for name in ["numpy", "black", "pytest", "bla"] {
            assert!(!manifest.contains(&format!("{name} =")));
        }
    }

    #[tokio::test]
    async fn ambiguous_dependency_reports_locations_without_editing() {
        let workspace = workspace_from(
            r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]

[dependencies]
ruff = "*"

[feature.dev.pypi-dependencies]
ruff = "*"
"#,
        );
        let manifest_path = workspace.workspace.provenance.path.clone();
        let original = fs_err::read_to_string(&manifest_path).unwrap();

        let err = remove_deps_unqualified(
            workspace.modify().unwrap(),
            vec!["ruff".to_string()],
            options(),
        )
        .await
        .unwrap_err();

        insta::assert_snapshot!(
            format_diagnostic(&err),
            @"  × dependency `ruff` exists in multiple locations: dependencies in the default feature, pypi-dependencies in feature `dev`"
        );
        assert_eq!(fs_err::read_to_string(manifest_path).unwrap(), original);
    }

    #[tokio::test]
    async fn missing_dependency_does_not_partially_edit_manifest() {
        let workspace = workspace_from(
            r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]

[dependencies]
numpy = "*"
"#,
        );
        let manifest_path = workspace.workspace.provenance.path.clone();
        let original = fs_err::read_to_string(&manifest_path).unwrap();

        let err = remove_deps_unqualified(
            workspace.modify().unwrap(),
            vec!["numpy".to_string(), "missing".to_string()],
            options(),
        )
        .await
        .unwrap_err();

        assert!(matches!(err, RemoveError::NotFound { name } if name == "missing"));
        assert_eq!(fs_err::read_to_string(manifest_path).unwrap(), original);
    }

    #[tokio::test]
    async fn pypi_names_are_matched_but_removed_with_source_spelling() {
        let workspace = workspace_from(
            r#"
[workspace]
name = "test"
channels = []
platforms = ["linux-64"]

[pypi-dependencies]
foo_bar = "*"
"#,
        );
        let manifest_path = workspace.workspace.provenance.path.clone();

        remove_deps_unqualified(
            workspace.modify().unwrap(),
            vec!["Foo-Bar".to_string()],
            options(),
        )
        .await
        .unwrap();

        assert!(
            !fs_err::read_to_string(manifest_path)
                .unwrap()
                .contains("foo_bar")
        );
    }

    #[tokio::test]
    async fn removes_native_pyproject_dependency() {
        let workspace = workspace_from_pyproject(
            r#"
[project]
name = "test"
version = "0.1.0"
requires-python = ">=3.11"
dependencies = ["Requests>=2"]

[tool.pixi.workspace]
channels = []
platforms = ["linux-64"]
"#,
        );
        let manifest_path = workspace.workspace.provenance.path.clone();

        remove_deps_unqualified(
            workspace.modify().unwrap(),
            vec!["requests".to_string()],
            options(),
        )
        .await
        .unwrap();

        assert!(
            !fs_err::read_to_string(manifest_path)
                .unwrap()
                .contains("Requests>=2")
        );
    }
}
