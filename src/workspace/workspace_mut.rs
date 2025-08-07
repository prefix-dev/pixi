use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
    sync::Arc,
};

use indexmap::IndexMap;
use itertools::Itertools;
use miette::{IntoDiagnostic, NamedSource};
use pep440_rs::VersionSpecifiers;
use pep508_rs::{Requirement, VersionOrUrl::VersionSpecifier};
use pixi_config::PinningStrategy;
use pixi_manifest::{
    DependencyOverwriteBehavior, FeatureName, FeaturesExt, HasFeaturesIter, LoadManifestsError,
    ManifestDocument, ManifestKind, PypiDependencyLocation, SpecType, TomlError, WorkspaceManifest,
    WorkspaceManifestMut, toml::TomlDocument, utils::WithSourceCode,
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, NamelessMatchSpec, PackageName, Platform, Version};
use rattler_lock::LockFile;
use toml_edit::DocumentMut;

use crate::{
    Workspace,
    cli::cli_config::{LockFileUpdateConfig, PrefixUpdateConfig},
    diff::LockFileDiff,
    environment::LockFileUsage,
    lock_file::{LockFileDerivedData, ReinstallPackages, UpdateContext, UpdateMode},
    workspace::{
        MatchSpecs, NON_SEMVER_PACKAGES, PypiDeps, SourceSpecs, UpdateDeps,
        grouped_environment::GroupedEnvironment,
    },
};

struct OriginalContent {
    manifest: WorkspaceManifest,
    source: String,
}

/// This struct represents a safe mutable representation of a [`Workspace`].
///
/// It offers methods to mutate the in-memory [`Workspace`] together with the
/// TOML representation of the manifest files it wraps.
///
/// A [`Workspace`] does not contain the original source code from which it is
/// derived. It only contains references to the files on disk. This struct,
/// however, parses the original source code into a TOMl representation which
/// can then be modified.
///
/// You can turn a non-mutable workspace into a [`WorkspaceMut`] by calling
/// [`Workspace::modify`]. This function will consume the original workspace and
/// return a mutable workspace.
///
/// Any changes made to this struct are *not* persisted until the
/// [`WorkspaceMut::save`] method is called. If the changes should be reverted
/// to the original state (in case of an error for instance) the
/// [`WorkspaceMut::revert`] method can be used. If the struct is dropped
/// without calling either [`WorkspaceMut::save`] or [`WorkspaceMut::revert`]
/// the changes are also reverted.
pub struct WorkspaceMut {
    // This is an option to indicate whether this instance has been consumed. Both `save` and
    // `revert` return the original workspace from which this instance was created and return this
    // field. However, the drop function still needs to be run afterward which cannot happen if we
    // already consumed this field. To facilitate this, we use an option from which the `Workspace`
    // is "taken" when consumed while keeping this instance in a valid state.
    workspace: Option<Workspace>,

    // The original manifest and string content of the manifest on disk. This
    // is used when reverting changes.
    original: Option<OriginalContent>,

    // Defines whether the content on disk has been modified. E.g. whether
    // there are intermediate changes that have not been saved.
    modified: bool,

    // The parsed toml document.
    workspace_manifest_document: ManifestDocument,
}

impl WorkspaceMut {
    /// Constructs a new [`WorkspaceMut`] by lightly parsing the files from
    /// which the workspace was created.
    ///
    /// Prefer to use [`Workspace::modify`] over this function.
    pub(super) fn new(workspace: Workspace) -> Result<Self, LoadManifestsError> {
        // Read the contents of the file
        let contents = workspace.workspace.provenance.read()?.into_inner();

        // Parse the contents
        let toml = match DocumentMut::from_str(&contents) {
            Ok(document) => TomlDocument::new(document),
            Err(err) => {
                return Err(Box::new(WithSourceCode {
                    source: NamedSource::new(
                        workspace.workspace.provenance.path.to_string_lossy(),
                        Arc::from(contents),
                    ),
                    error: TomlError::from(err),
                })
                .into());
            }
        };

        let workspace_manifest_document = match workspace.workspace.provenance.kind {
            ManifestKind::Pyproject => ManifestDocument::PyProjectToml(toml),
            ManifestKind::Pixi => ManifestDocument::PixiToml(toml),
            ManifestKind::MojoProject => ManifestDocument::MojoProjectToml(toml),
        };

        Ok(Self {
            original: Some(OriginalContent {
                manifest: workspace.workspace.value.clone(),
                source: contents.clone(),
            }),
            modified: false,

            workspace: Some(workspace),
            workspace_manifest_document,
        })
    }

    /// Constructs a new workspace from the contents of a string. This
    /// workspace has no backing on disk until saved.
    pub fn from_template(
        manifest_path: PathBuf,
        contents: String,
    ) -> Result<Self, LoadManifestsError> {
        // Parse the document directly from the string.
        let toml = match DocumentMut::from_str(&contents) {
            Ok(document) => TomlDocument::new(document),
            Err(err) => {
                return Err(Box::new(WithSourceCode {
                    source: NamedSource::new(manifest_path.to_string_lossy(), Arc::from(contents)),
                    error: TomlError::from(err),
                })
                .into());
            }
        };

        // Parse the content into a workspace
        let workspace = Workspace::from_str(&manifest_path, &contents)?;

        let workspace_manifest_document = match workspace.workspace.provenance.kind {
            ManifestKind::Pyproject => ManifestDocument::PyProjectToml(toml),
            ManifestKind::Pixi => ManifestDocument::PixiToml(toml),
            ManifestKind::MojoProject => ManifestDocument::MojoProjectToml(toml),
        };

        Ok(Self {
            original: None,
            modified: true, // The content is not on disk yet, so we are in a modified state.

            workspace: Some(workspace),
            workspace_manifest_document,
        })
    }

    /// Returns the kind of manifest this workspace is derived from.
    fn kind(&self) -> ManifestKind {
        self.workspace_manifest_document.kind()
    }

    /// Returns a [`WorkspaceManifestMut`] which implements methods to modify a
    /// workspace manifest both in memory and on-disk.
    #[must_use]
    pub fn manifest(&mut self) -> WorkspaceManifestMut<'_> {
        WorkspaceManifestMut {
            workspace: &mut self
                .workspace
                .as_mut()
                .expect("workspace is not available")
                .workspace
                .value,
            document: &mut self.workspace_manifest_document,
        }
    }

    /// Returns a reference to the in-memory representation of the workspace.
    ///
    /// Any previous changes made to the workspace are reflected in the returned
    /// value.
    pub fn workspace(&self) -> &Workspace {
        self.workspace.as_ref().expect("workspace is not available")
    }

    /// Returns a reference to the underlying workspace manifest document.
    pub fn document(&self) -> &ManifestDocument {
        &self.workspace_manifest_document
    }

    /// An internal method to save the changes to the workspace manifest to disk
    /// without consuming the instance.
    ///
    /// This is useful if an operation needs to save the changes but still needs
    /// to continue the modification.
    async fn save_inner(&mut self) -> Result<(), std::io::Error> {
        let new_contents = self.workspace_manifest_document.to_string();
        fs_err::tokio::write(&self.workspace().workspace.provenance.path, new_contents).await?;
        self.modified = true;
        Ok(())
    }

    /// Save the changes to the workspace manifest to disk and return the
    /// modified [`Workspace`].
    pub async fn save(mut self) -> Result<Workspace, std::io::Error> {
        self.save_inner().await?;
        Ok(self.workspace.take().expect("workspace is not available"))
    }

    /// Revert the changes made to the workspace manifest and returns the
    /// unmodified [`Workspace`].
    pub async fn revert(mut self) -> Result<Workspace, std::io::Error> {
        let mut workspace = self.workspace.take().expect("workspace is not available");
        if let Some(original) = self.original.take() {
            workspace.workspace.value = original.manifest;
            fs_err::tokio::write(&workspace.workspace.provenance.path, original.source).await?;
        }

        Ok(workspace)
    }

    /// Update the manifest with the given package specs, and upgrade the
    /// packages if possible
    ///
    /// 1. Modify the manifest with the given package specs, if no version is
    ///    given, use `no-pin` strategy
    /// 2. Update the lock file
    /// 3. Given packages without version restrictions will get a semver
    ///    restriction
    #[allow(clippy::too_many_arguments)]
    pub async fn update_dependencies(
        &mut self,
        match_specs: MatchSpecs,
        pypi_deps: PypiDeps,
        source_specs: SourceSpecs,
        prefix_update_config: &PrefixUpdateConfig,
        lock_file_update_config: &LockFileUpdateConfig,
        feature_name: &FeatureName,
        platforms: &[Platform],
        editable: bool,
        dry_run: bool,
    ) -> Result<Option<UpdateDeps>, miette::Error> {
        let mut conda_specs_to_add_constraints_for = IndexMap::new();
        let mut pypi_specs_to_add_constraints_for = IndexMap::new();
        let mut conda_packages = HashSet::new();
        let mut pypi_packages = HashSet::new();
        let channel_config = self.workspace().channel_config();
        for (name, (spec, spec_type)) in match_specs {
            let (_, nameless_spec) = spec.into_nameless();
            let pixi_spec =
                PixiSpec::from_nameless_matchspec(nameless_spec.clone(), &channel_config);

            let added = self.manifest().add_dependency(
                &name,
                &pixi_spec,
                spec_type,
                platforms,
                feature_name,
                DependencyOverwriteBehavior::Overwrite,
            )?;
            if added {
                if nameless_spec.version.is_none() {
                    conda_specs_to_add_constraints_for
                        .insert(name.clone(), (spec_type, nameless_spec));
                }
                conda_packages.insert(name);
            }
        }

        for (name, (spec, spec_type)) in source_specs {
            let pixi_spec = PixiSpec::from(spec);

            self.manifest().add_dependency(
                &name,
                &pixi_spec,
                spec_type,
                platforms,
                feature_name,
                DependencyOverwriteBehavior::Overwrite,
            )?;
        }

        for (name, (spec, pixi_spec, location)) in pypi_deps {
            let added = self.manifest().add_pep508_dependency(
                (&spec, pixi_spec.as_ref()),
                platforms,
                feature_name,
                Some(editable),
                DependencyOverwriteBehavior::Overwrite,
                location.as_ref(),
            )?;
            if added {
                if spec.version_or_url.is_none() {
                    pypi_specs_to_add_constraints_for
                        .insert(name.clone(), (spec, pixi_spec, location));
                }
                pypi_packages.insert(name.as_normalized().clone());
            }
        }

        // Only save the project if it is a pyproject.toml
        // This is required to ensure that the changes are found by tools like `pixi
        // build` and `uv`
        if self.kind() == ManifestKind::Pyproject {
            self.save_inner().await.into_diagnostic()?;
        }

        if lock_file_update_config.lock_file_usage()? != LockFileUsage::Update {
            return Ok(None);
        }

        let original_lock_file = self.workspace().load_lock_file().await?;
        let affected_environments = self
            .workspace()
            .environments()
            .iter()
            // Filter out any environment that does not contain the feature we modified
            .filter(|e| e.features().any(|f| f.name == *feature_name))
            // Expand the selection to also included any environment that shares the same solve
            // group
            .flat_map(|e| {
                GroupedEnvironment::from(e.clone())
                    .environments()
                    .collect_vec()
            })
            .unique()
            .collect_vec();
        let default_environment_is_affected =
            affected_environments.contains(&self.workspace().default_environment());
        tracing::debug!(
            "environments affected by the add command: {}",
            affected_environments.iter().map(|e| e.name()).format(", ")
        );
        let affect_environment_and_platforms = affected_environments
            .into_iter()
            // Create an iterator over all environment and platform combinations
            .flat_map(|e| e.platforms().into_iter().map(move |p| (e.clone(), p)))
            // Filter out any platform that is not affected by the changes.
            .filter(|(_, platform)| platforms.is_empty() || platforms.contains(platform))
            .map(|(e, p)| (e.name().to_string(), p))
            .collect_vec();
        let unlocked_lock_file = self.workspace().unlock_packages(
            &original_lock_file,
            conda_packages,
            pypi_packages,
            affect_environment_and_platforms
                .iter()
                .map(|(e, p)| (e.as_str(), *p))
                .collect(),
        );
        let LockFileDerivedData {
            workspace: _, // We don't need the project here
            lock_file,
            package_cache,
            uv_context,
            updated_conda_prefixes,
            updated_pypi_prefixes,
            command_dispatcher,
            glob_hash_cache,
            io_concurrency_limit,
            was_outdated: _,
        } = UpdateContext::builder(self.workspace())
            .with_lock_file(unlocked_lock_file)
            .with_no_install(
                (prefix_update_config.no_install && lock_file_update_config.no_lockfile_update)
                    || dry_run,
            )
            .finish()
            .await?
            .update()
            .await?;

        let mut implicit_constraints = HashMap::new();
        if !conda_specs_to_add_constraints_for.is_empty() {
            let conda_constraints = self.update_conda_specs_from_lock_file(
                &lock_file,
                conda_specs_to_add_constraints_for,
                affect_environment_and_platforms.clone(),
                feature_name,
                platforms,
            )?;
            implicit_constraints.extend(conda_constraints);
        }

        if !pypi_specs_to_add_constraints_for.is_empty() {
            let pypi_constraints = self.update_pypi_specs_from_lock_file(
                &lock_file,
                pypi_specs_to_add_constraints_for,
                affect_environment_and_platforms,
                feature_name,
                platforms,
                editable,
            )?;
            implicit_constraints.extend(pypi_constraints);
        }

        // Only save the project if it is a pyproject.toml
        // This is required to ensure that the changes are found by tools like `pixi
        // build` and `uv`
        if self.kind() == ManifestKind::Pyproject {
            self.save_inner().await.into_diagnostic()?;
        }

        let updated_lock_file = LockFileDerivedData {
            workspace: self.workspace(),
            lock_file,
            package_cache,
            updated_conda_prefixes,
            updated_pypi_prefixes,
            uv_context,
            io_concurrency_limit,
            command_dispatcher,
            glob_hash_cache,
            was_outdated: true,
        };
        if !lock_file_update_config.no_lockfile_update && !dry_run {
            updated_lock_file.write_to_disk()?;
        }
        if !prefix_update_config.no_install
            && !lock_file_update_config.no_lockfile_update
            && !dry_run
            && self.workspace().environments().len() == 1
            && default_environment_is_affected
        {
            updated_lock_file
                .prefix(
                    &self.workspace().default_environment(),
                    UpdateMode::Revalidate,
                    &ReinstallPackages::default(),
                    &[],
                )
                .await?;
        }

        let lock_file_diff =
            LockFileDiff::from_lock_files(&original_lock_file, &updated_lock_file.into_lock_file());

        Ok(Some(UpdateDeps {
            implicit_constraints,
            lock_file_diff,
        }))
    }

    // Take some conda and PyPI deps as Vecs of MatchSpecs and Requirements, and add them
    // for given platforms and a given feature
    pub fn add_specs(
        &mut self,
        conda_deps: Vec<MatchSpec>,
        pypi_deps: Vec<Requirement>,
        platforms: &[Platform],
        feature_name: &FeatureName,
    ) -> Result<(), miette::Error> {
        for spec in conda_deps {
            // Determine the name of the package to add
            let (Some(name), spec) = spec.clone().into_nameless() else {
                miette::bail!(
                    "{} does not support wildcard dependencies",
                    pixi_utils::executable_name()
                );
            };
            let spec = PixiSpec::from_nameless_matchspec(spec, &self.workspace().channel_config());
            self.manifest().add_dependency(
                &name,
                &spec,
                SpecType::Run,
                // No platforms required as you can't define them in the yaml
                platforms,
                feature_name,
                DependencyOverwriteBehavior::Overwrite,
            )?;
        }
        for requirement in pypi_deps {
            self.manifest().add_pep508_dependency(
                (&requirement, None),
                // No platforms required as you can't define them in the yaml
                platforms,
                feature_name,
                None,
                DependencyOverwriteBehavior::Overwrite,
                None,
            )?;
        }
        Ok(())
    }

    /// Update the conda specs of newly added packages based on the contents of
    /// the updated lock-file.
    fn update_conda_specs_from_lock_file(
        &mut self,
        updated_lock_file: &LockFile,
        conda_specs_to_add_constraints_for: IndexMap<PackageName, (SpecType, NamelessMatchSpec)>,
        affect_environment_and_platforms: Vec<(String, Platform)>,
        feature_name: &FeatureName,
        platforms: &[Platform],
    ) -> miette::Result<HashMap<String, String>> {
        let mut implicit_constraints = HashMap::new();

        // Determine the conda records that were affected by the add.
        let conda_records = affect_environment_and_platforms
            .into_iter()
            // Get all the conda and pypi records for the combination of environments and
            // platforms
            .filter_map(|(env, platform)| {
                let locked_env = updated_lock_file.environment(&env)?;
                locked_env.conda_repodata_records(platform).ok()?
            })
            .flatten()
            .collect_vec();

        let channel_config = self.workspace().channel_config();
        for (name, (spec_type, spec)) in conda_specs_to_add_constraints_for {
            let mut pinning_strategy = self.workspace().config().pinning_strategy;

            // Edge case: some packages are a special case where we want to pin the minor
            // version by default. This is done to avoid early user confusion
            // when the minor version changes and environments magically start breaking.
            // This move a `>=3.13, <4` to a `>=3.13, <3.14` constraint.
            if NON_SEMVER_PACKAGES.contains(&name.as_normalized()) && pinning_strategy.is_none() {
                tracing::info!(
                    "Pinning {} to minor version by default",
                    name.as_normalized()
                );
                pinning_strategy = Some(PinningStrategy::Minor);
            }
            let version_constraint = pinning_strategy
                .unwrap_or_default()
                .determine_version_constraint(conda_records.iter().filter_map(|record| {
                    if record.package_record.name == name {
                        Some(record.package_record.version.version())
                    } else {
                        None
                    }
                }));

            if let Some(version_constraint) = version_constraint {
                implicit_constraints
                    .insert(name.as_source().to_string(), version_constraint.to_string());
                let spec = NamelessMatchSpec {
                    version: Some(version_constraint),
                    ..spec
                };

                let pixi_spec = PixiSpec::from_nameless_matchspec(spec.clone(), &channel_config);

                self.manifest().add_dependency(
                    &name,
                    &pixi_spec,
                    spec_type,
                    platforms,
                    feature_name,
                    DependencyOverwriteBehavior::Overwrite,
                )?;
            }
        }

        Ok(implicit_constraints)
    }

    /// Update the pypi specs of newly added packages based on the contents of
    /// the updated lock-file.
    fn update_pypi_specs_from_lock_file(
        &mut self,
        updated_lock_file: &LockFile,
        pypi_specs_to_add_constraints_for: IndexMap<
            PypiPackageName,
            (
                Requirement,
                Option<PixiPypiSpec>,
                Option<PypiDependencyLocation>,
            ),
        >,
        affect_environment_and_platforms: Vec<(String, Platform)>,
        feature_name: &FeatureName,
        platforms: &[Platform],
        editable: bool,
    ) -> miette::Result<HashMap<String, String>> {
        let mut implicit_constraints = HashMap::new();

        let affect_environment_and_platforms = affect_environment_and_platforms
            .iter()
            .filter_map(|(env, platform)| {
                updated_lock_file.environment(env).map(|e| (e, *platform))
            })
            .collect_vec();

        let pypi_records = affect_environment_and_platforms
            // Get all the conda and pypi records for the combination of environments and
            // platforms
            .iter()
            .filter_map(|(env, platform)| env.pypi_packages(*platform))
            .flatten()
            .collect_vec();

        let pinning_strategy = self
            .workspace()
            .config()
            .pinning_strategy
            .unwrap_or_default();

        // Determine the versions of the packages in the lock-file
        for (name, (req, pixi_req, location)) in pypi_specs_to_add_constraints_for {
            let version_constraint = pinning_strategy.determine_version_constraint(
                pypi_records
                    .iter()
                    .filter_map(|(data, _)| {
                        if &data.name == name.as_normalized() {
                            Version::from_str(&data.version.to_string()).ok()
                        } else {
                            None
                        }
                    })
                    .collect_vec()
                    .iter(),
            );

            let version_spec = version_constraint
                .and_then(|spec| VersionSpecifiers::from_str(&spec.to_string()).ok());
            if let Some(version_spec) = version_spec {
                implicit_constraints.insert(name.as_source().to_string(), version_spec.to_string());
                let req = Requirement {
                    version_or_url: Some(VersionSpecifier(version_spec)),
                    ..req
                };

                self.manifest().add_pep508_dependency(
                    (&req, pixi_req.as_ref()),
                    platforms,
                    feature_name,
                    Some(editable),
                    DependencyOverwriteBehavior::Overwrite,
                    location.as_ref(),
                )?;
            }
        }

        Ok(implicit_constraints)
    }
}

impl Drop for WorkspaceMut {
    fn drop(&mut self) {
        if let (Some(workspace), Some(original)) = (self.workspace.take(), self.original.take()) {
            if self.modified {
                let path = workspace.workspace.provenance.path;
                if let Err(err) = fs_err::write(&path, &original.source) {
                    tracing::error!(
                        "Failed to revert manifest changes to {}: {}",
                        path.display(),
                        err
                    );
                }
            }
        }
    }
}
