use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use pixi_core::Workspace;
use pixi_manifest::{Feature, FeatureName, PrioritizedChannel, TargetSelector, Task, TaskName};
use pixi_spec::PixiSpec;
use rattler_conda_types::PackageName;

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
