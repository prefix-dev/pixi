use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use miette::IntoDiagnostic;
use pixi_core::{Workspace, workspace::WorkspaceMut};
use pixi_manifest::{
    EnvironmentName, Feature, FeatureName, HasFeaturesIter, PrioritizedChannel, TargetSelector,
    Task, TaskName,
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;

use crate::{Interface, workspace::workspace::environment};

pub async fn list_features(workspace: &Workspace) -> IndexMap<FeatureName, Feature> {
    workspace.workspace.value.features.clone()
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
) -> miette::Result<Vec<EnvironmentName>> {
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
    workspace.save().await.into_diagnostic()?;

    interface
        .success(&format!("Removed feature {feature}"))
        .await;

    Ok(modified_envs)
}
