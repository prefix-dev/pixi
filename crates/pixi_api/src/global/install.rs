use std::ops::Not;

use itertools::Itertools;
use rattler_conda_types::{MatchSpec, NamedChannelOrUrl, Subdir};
use serde::{Deserialize, Serialize};

use pixi_global::{
    EnvironmentName, Mapping, Project, StateChange, StateChanges,
    common::{GlobalDirError, contains_menuinst_document},
    project::{
        EnvironmentInSyncError, ExposeError, ExposedType, FromMatchSpecError, GlobalSpec,
        InstallEnvironmentError, ManifestError, RemoveEnvironmentError, ShortcutsError,
    },
};
use pixi_utils::prefix::PrefixError;

use crate::global::utils::{eventual_environment_channels, revert_environment_after_error};

/// An error from installing packages into a global environment.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum InstallError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Manifest(#[from] ManifestError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    RemoveEnvironment(#[from] Box<RemoveEnvironmentError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    FromMatchSpec(#[from] FromMatchSpecError),

    #[error("could not find exact package name in MatchSpec {0}")]
    MissingPackageName(String),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InSync(#[from] EnvironmentInSyncError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    InstallEnvironment(#[from] InstallEnvironmentError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Expose(#[from] ExposeError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    GlobalDir(#[from] GlobalDirError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Prefix(#[from] PrefixError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Shortcuts(#[from] ShortcutsError),
}

/// Options for installing packages into a global environment.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstallOptions {
    /// The channels of a newly created environment. If empty, the default
    /// channels of the config are used.
    pub channels: Vec<NamedChannelOrUrl>,

    /// The platform to install the packages for.
    pub platform: Option<Subdir>,

    /// Mappings which describe which executables are exposed. If empty, all
    /// executables of the installed packages are exposed.
    pub expose: Vec<Mapping>,

    /// Additional dependencies whose executables are not exposed.
    pub with: Vec<MatchSpec>,

    /// Whether the environment should be reinstalled.
    pub force_reinstall: bool,

    /// Whether no shortcuts should be created for the installed packages.
    pub no_shortcuts: bool,
}

/// Install `specs` into the environment `env_name`. On failure the
/// environment is reverted and `project` is left untouched.
pub async fn install_environment(
    project: &mut Project,
    env_name: &EnvironmentName,
    specs: &[GlobalSpec],
    options: &InstallOptions,
) -> Result<StateChanges, InstallError> {
    let mut modified_project = project.clone();
    match setup_environment(env_name, options, specs, &mut modified_project).await {
        Ok(state_changes) => {
            // Only advance project on success
            *project = modified_project;
            Ok(state_changes)
        }
        Err(err) => {
            revert_environment_after_error(env_name, project).await;
            Err(err)
        }
    }
}

async fn setup_environment(
    env_name: &EnvironmentName,
    options: &InstallOptions,
    specs: &[GlobalSpec],
    project: &mut Project,
) -> Result<StateChanges, InstallError> {
    let mut state_changes = StateChanges::new_with_env(env_name.clone());

    // The manifest as it was before this environment was touched, so the
    // shortcut below can tell whether it still owes the file a write.
    let manifest_before = project.manifest.document.to_string();

    if options.force_reinstall && project.environment(env_name).is_some() {
        state_changes |= project
            .remove_environment(env_name)
            .await
            .map_err(Box::new)?;
    }

    let channels = eventual_environment_channels(
        project,
        Some(env_name),
        &options.channels,
        options.force_reinstall,
    );

    // Modify the project to include the new environment
    if !project.manifest.parsed.envs.contains_key(env_name) {
        let channels = channels
            .into_iter()
            .map(|channel| channel.channel)
            .collect();
        project.manifest.add_environment(env_name, Some(channels))?;
        state_changes.insert_change(env_name, StateChange::AddedEnvironment);
    }

    if let Some(platform) = options.platform {
        project.manifest.set_platform(env_name, platform)?;
    }

    let converted_with_inclusions = options
        .with
        .iter()
        .map(|spec| {
            GlobalSpec::try_from_matchspec_with_name(
                spec.clone(),
                project.config().global_channel_config(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    // Add the dependencies to the environment
    let packages_to_add = specs
        .iter()
        .chain(converted_with_inclusions.iter())
        .collect_vec();

    for spec in &packages_to_add {
        project.manifest.add_dependency(env_name, spec)?;
    }

    if !options.expose.is_empty() {
        project.manifest.remove_all_exposed_mappings(env_name)?;
        // Only add the exposed mappings that were requested
        for mapping in &options.expose {
            project.manifest.add_exposed_mapping(env_name, mapping)?;
        }
    }

    // The prefix already satisfies the manifest, so there is nothing to solve,
    // install or expose. The manifest itself can still have changed: it may
    // have just gained the environment that the prefix was left over from, and
    // that change still has to be written out.
    if project.environment_in_sync_internal(env_name, true).await? {
        if project.manifest.document.to_string() != manifest_before {
            project.manifest.save().await?;
        }
        return Ok(state_changes);
    }

    // Installing the environment to be able to find the bin paths later
    let environment_update = project
        .install_environment_with_options(env_name, options.force_reinstall)
        .await?;
    project.clear_progress();

    // Sync exposed name
    sync_exposed_names(env_name, project, options).await?;

    // Add shortcuts
    if !options.no_shortcuts {
        let prefix = project.environment_prefix(env_name).await?;
        for spec in specs.iter() {
            let prefix_record = prefix.find_designated_package(spec.name()).await?;
            if contains_menuinst_document(&prefix_record, prefix.root()) {
                project.manifest.add_shortcut(env_name, spec.name())?;
            }
        }
        state_changes |= project.sync_shortcuts(env_name).await?;
    }

    state_changes.insert_change(
        env_name,
        StateChange::UpdatedEnvironment(environment_update),
    );

    // Expose executables of the new environment
    state_changes |= project
        .expose_executables_from_environment(env_name)
        .await?;

    // Sync completions
    state_changes |= project.sync_completions(env_name).await?;

    project.manifest.save().await?;
    Ok(state_changes)
}

async fn sync_exposed_names(
    env_name: &EnvironmentName,
    project: &mut Project,
    options: &InstallOptions,
) -> Result<(), InstallError> {
    let with_package_names = options
        .with
        .iter()
        .map(|spec| {
            spec.name
                .as_exact()
                .cloned()
                .ok_or_else(|| InstallError::MissingPackageName(spec.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expose_type = if options.expose.is_empty().not() {
        ExposedType::Mappings(options.expose.clone())
    } else if with_package_names.is_empty() {
        ExposedType::All
    } else {
        ExposedType::Ignore(with_package_names)
    };
    project.sync_exposed_names(env_name, expose_type).await?;
    Ok(())
}
