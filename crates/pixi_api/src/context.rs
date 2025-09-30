use std::collections::HashMap;

use pixi_core::{Workspace, environment::LockFileUsage};
use pixi_manifest::{EnvironmentName, FeatureName, Task, TaskName};
use rattler_conda_types::Platform;

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

    pub async fn init(interface: I, options: InitOptions) -> miette::Result<Workspace> {
        crate::workspace::init::init(&interface, options).await
    }

    pub async fn name(&self) -> String {
        crate::workspace::workspace::name::get(&self.workspace).await
    }

    pub async fn set_name(&self, name: &str) -> miette::Result<()> {
        crate::workspace::workspace::name::set(&self.interface, &self.workspace, name).await
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
            &self.workspace,
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
        crate::workspace::task::alias_task(&self.interface, &self.workspace, name, task, platform)
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
            &self.workspace,
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
