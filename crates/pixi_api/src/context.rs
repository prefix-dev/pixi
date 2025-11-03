use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use miette::IntoDiagnostic;
use pixi_core::workspace::WorkspaceMut;
use pixi_core::{Workspace, environment::LockFileUsage};
use pixi_manifest::{
    EnvironmentName, Feature, FeatureName, PrioritizedChannel, TargetSelector, Task, TaskName,
};
use pixi_spec::PixiSpec;
use rattler_conda_types::{PackageName, Platform};

use crate::interface::Interface;
use crate::workspace::{InitOptions, ReinstallOptions};

pub struct WorkspaceContext<I: Interface> {
    interface: I,
    workspace: Workspace,
}

impl<I: Interface> WorkspaceContext<I> {
    pub fn new(interface: I, workspace: Workspace) -> Self {
        Self {
            interface,
            workspace,
        }
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn workspace_mut(&self) -> miette::Result<WorkspaceMut> {
        self.workspace.clone().modify().into_diagnostic()
    }

    pub async fn init(interface: I, options: InitOptions) -> miette::Result<Workspace> {
        crate::workspace::init::init(&interface, options).await
    }

    pub async fn name(&self) -> String {
        crate::workspace::workspace::name::get(&self.workspace).await
    }

    pub async fn set_name(&self, name: &str) -> miette::Result<()> {
        crate::workspace::workspace::name::set(&self.interface, self.workspace_mut()?, name).await
    }

    pub async fn list_features(&self) -> IndexMap<FeatureName, Feature> {
        crate::workspace::workspace::feature::list_features(&self.workspace).await
    }

    pub async fn list_feature_channels(
        &self,
        feature: FeatureName,
    ) -> Option<IndexSet<PrioritizedChannel>> {
        crate::workspace::workspace::feature::list_feature_channels(&self.workspace, feature).await
    }

    pub async fn list_feature_dependencies(
        &self,
        feature: FeatureName,
        target: Option<&TargetSelector>,
    ) -> Option<HashMap<PackageName, Vec<PixiSpec>>> {
        crate::workspace::workspace::feature::list_feature_dependencies(
            &self.workspace,
            feature,
            target,
        )
        .await
    }

    pub async fn list_feature_tasks(
        &self,
        feature: FeatureName,
        target: Option<&TargetSelector>,
    ) -> Option<HashMap<TaskName, Task>> {
        crate::workspace::workspace::feature::list_feature_tasks(&self.workspace, feature, target)
            .await
    }

    pub async fn list_tasks(
        &self,
        environment: Option<EnvironmentName>,
    ) -> miette::Result<HashMap<EnvironmentName, HashMap<TaskName, Task>>> {
        crate::workspace::task::list_tasks(&self.interface, &self.workspace, environment).await
    }

    pub async fn add_task(
        &self,
        name: TaskName,
        task: Task,
        feature: FeatureName,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        crate::workspace::task::add_task(
            &self.interface,
            self.workspace_mut()?,
            name,
            task,
            feature,
            platform,
        )
        .await
    }

    pub async fn alias_task(
        &self,
        name: TaskName,
        task: Task,
        platform: Option<Platform>,
    ) -> miette::Result<()> {
        crate::workspace::task::alias_task(
            &self.interface,
            self.workspace_mut()?,
            name,
            task,
            platform,
        )
        .await
    }

    pub async fn remove_task(
        &self,
        names: Vec<TaskName>,
        platform: Option<Platform>,
        feature: FeatureName,
    ) -> miette::Result<()> {
        crate::workspace::task::remove_tasks(
            &self.interface,
            self.workspace_mut()?,
            names,
            platform,
            feature,
        )
        .await
    }

    pub async fn reinstall(
        &self,
        options: ReinstallOptions,
        lock_file_usage: LockFileUsage,
    ) -> miette::Result<()> {
        crate::workspace::reinstall::reinstall(
            &self.interface,
            &self.workspace,
            options,
            lock_file_usage,
        )
        .await
    }
}
