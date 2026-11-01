use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use miette::Diagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::{
    EnvironmentEditError, EnvironmentName, Feature, FeatureName, HasFeaturesIter,
    LoadManifestsError, PrioritizedChannel, TargetSelector, Task, TaskName,
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;

use thiserror::Error;

use crate::workspace::workspace::{SaveManifestError, environment, save};
use crate::{Interface, InterfaceError};

#[derive(Debug, Error, Diagnostic)]
pub enum RemoveFeatureError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    LoadWorkspace(#[from] LoadManifestsError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Interface(#[from] InterfaceError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Edit(#[from] EnvironmentEditError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Save(#[from] SaveManifestError),
}

pub async fn list_features(workspace: &Workspace) -> IndexMap<FeatureName, Feature> {
    workspace
        .workspace
        .value
        .user_features()
        .map(|(name, feature)| (name.clone(), feature.clone()))
        .collect()
}

pub async fn list_feature_channels(
    workspace: &Workspace,
    feature: FeatureName,
) -> Option<IndexSet<PrioritizedChannel>> {
    workspace
        .workspace
        .value
        .feature(&feature)
        .and_then(|f| f.channels.clone())
}

pub async fn list_feature_dependencies(
    workspace: &Workspace,
    feature: FeatureName,
    target: Option<&TargetSelector>,
) -> Option<HashMap<PackageName, Vec<PixiSpec>>> {
    workspace.workspace.value.feature(&feature).and_then(|f| {
        f.targets
            .for_opt_target(target)
            .and_then(|t| t.run_dependencies())
            .map(|deps| {
                deps.iter()
                    .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                    .collect()
            })
    })
}

pub async fn list_feature_pypi_dependencies(
    workspace: &Workspace,
    feature: FeatureName,
    target: Option<&TargetSelector>,
) -> Option<HashMap<PypiPackageName, Vec<PixiPypiSpec>>> {
    workspace.workspace.value.feature(&feature).and_then(|f| {
        f.targets
            .for_opt_target(target)
            .and_then(|t| t.pypi_dependencies.as_ref())
            .map(|deps| {
                deps.iter()
                    .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
                    .collect()
            })
    })
}

pub async fn list_feature_tasks(
    workspace: &Workspace,
    feature: FeatureName,
    target: Option<&TargetSelector>,
) -> Option<HashMap<TaskName, Task>> {
    workspace.workspace.value.feature(&feature).and_then(|f| {
        f.targets
            .for_opt_target(target)
            .map(|target| target.tasks.clone())
    })
}

pub async fn feature_by_task(
    workspace: &Workspace,
    task: &TaskName,
    environment: &EnvironmentName,
) -> Option<FeatureName> {
    let environment = workspace.environment(environment)?;
    let feature_tasks = environment.feature_tasks();

    for (feature_name, tasks) in feature_tasks {
        if tasks.contains_key(task) {
            return Some(feature_name.clone());
        }
    }

    None
}

pub async fn remove_feature<I: Interface>(
    interface: &I,
    mut workspace: WorkspaceMut,
    feature: &FeatureName,
) -> Result<Vec<EnvironmentName>, RemoveFeatureError> {
    // Check which environments use this feature
    let environments_using_feature: Vec<String> = environment::list(workspace.workspace())
        .await
        .into_iter()
        .filter(|env| env.features().any(|f| f.name == *feature))
        .map(|env| env.name().to_string())
        .collect();

    // If the feature is used in environments, ask for confirmation
    if !environments_using_feature.is_empty() {
        let message = if environments_using_feature.len() == 1 {
            format!(
                "Feature '{}' is used by environment '{}'. Do you want to remove it anyway?",
                feature, environments_using_feature[0]
            )
        } else {
            format!(
                "Feature '{}' is used by the following environments: {}. Do you want to remove it anyway?",
                feature,
                environments_using_feature.join(", ")
            )
        };

        let confirmed = interface.confirm(&message).await?;

        if !confirmed {
            return Ok(Vec::new());
        }
    }

    // Remove the feature
    let modified_envs = workspace.manifest().remove_feature(feature)?;
    save(workspace).await?;

    interface
        .success(&format!("Removed feature {feature}"))
        .await;

    Ok(modified_envs)
}
