use std::collections::HashMap;

use indexmap::{IndexMap, IndexSet};
use miette::IntoDiagnostic;
use pixi_core::workspace::{Environment, PypiDeps, UpdateDeps, WorkspaceMut};
use pixi_core::{Workspace, environment::LockFileUsage};
use pixi_manifest::{
    EnvironmentName, Feature, FeatureName, PrioritizedChannel, SpecType, TargetSelector, Task,
    TaskName,
};
use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    Channel, MatchSpec, NamedChannelOrUrl, PackageName, Platform, RepoDataRecord,
};

use crate::interface::Interface;
use crate::workspace::add::GitOptions;
use crate::workspace::{ChannelOptions, DependencyOptions, InitOptions, ReinstallOptions};

pub struct DefaultContext<I: Interface> {
    _interface: I,
}

impl<I: Interface> DefaultContext<I> {
    pub fn new(interface: I) -> Self {
        Self {
            _interface: interface,
        }
    }

    /// Returns all matching package versions sorted by version
    pub async fn search_exact(
        &self,
        match_spec: MatchSpec,
        channels: IndexSet<Channel>,
        platform: Platform,
    ) -> miette::Result<Option<Vec<RepoDataRecord>>> {
        crate::workspace::search::search_exact(None, match_spec, channels, platform).await
    }

    /// Returns all matching packages with their latest versions
    pub async fn search_wildcard(
        &self,
        search: &str,
        channels: IndexSet<Channel>,
        platform: Platform,
    ) -> miette::Result<Option<Vec<RepoDataRecord>>> {
        crate::workspace::search::search_wildcard(None, search, channels, platform).await
    }
}

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

    pub async fn description(&self) -> Option<String> {
        crate::workspace::workspace::description::get(&self.workspace).await
    }

    pub async fn set_description(&self, description: &str) -> miette::Result<()> {
        crate::workspace::workspace::description::set(
            &self.interface,
            self.workspace_mut()?,
            description,
        )
        .await
    }

    pub async fn list_channel(&self) -> HashMap<EnvironmentName, Vec<NamedChannelOrUrl>> {
        crate::workspace::workspace::channel::list(&self.workspace).await
    }

    pub async fn add_channel(
        &self,
        options: ChannelOptions,
        priority: Option<i32>,
        prepend: bool,
    ) -> miette::Result<()> {
        crate::workspace::workspace::channel::add(
            &self.interface,
            self.workspace_mut()?,
            options,
            priority,
            prepend,
        )
        .await
    }

    pub async fn remove_channel(
        &self,
        options: ChannelOptions,
        priority: Option<i32>,
    ) -> miette::Result<()> {
        crate::workspace::workspace::channel::remove(
            &self.interface,
            self.workspace_mut()?,
            options,
            priority,
        )
        .await
    }

    pub async fn set_channels(&self, options: ChannelOptions) -> miette::Result<()> {
        crate::workspace::workspace::channel::set(&self.interface, self.workspace_mut()?, options)
            .await
    }

    pub async fn list_platforms(&self) -> HashMap<EnvironmentName, Vec<Platform>> {
        crate::workspace::workspace::platform::list(&self.workspace).await
    }

    pub async fn add_platforms(
        &self,
        platform: Vec<Platform>,
        no_install: bool,
        feature: Option<String>,
    ) -> miette::Result<()> {
        crate::workspace::workspace::platform::add(
            &self.interface,
            self.workspace_mut()?,
            platform,
            no_install,
            feature,
        )
        .await
    }

    pub async fn remove_platforms(
        &self,
        platform: Vec<Platform>,
        no_install: bool,
        feature: Option<String>,
    ) -> miette::Result<()> {
        crate::workspace::workspace::platform::remove(
            &self.interface,
            self.workspace_mut()?,
            platform,
            no_install,
            feature,
        )
        .await
    }

    pub async fn list_environments(&self) -> Vec<Environment<'_>> {
        crate::workspace::workspace::environment::list(&self.workspace).await
    }

    pub async fn add_environment(
        &self,
        name: EnvironmentName,
        features: Option<Vec<String>>,
        solve_group: Option<String>,
        no_default_feature: bool,
        force: bool,
    ) -> miette::Result<()> {
        crate::workspace::workspace::environment::add(
            &self.interface,
            self.workspace_mut()?,
            name,
            features,
            solve_group,
            no_default_feature,
            force,
        )
        .await
    }

    pub async fn remove_environment(&self, name: &str) -> miette::Result<()> {
        crate::workspace::workspace::environment::remove(
            &self.interface,
            self.workspace_mut()?,
            name,
        )
        .await
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

    pub async fn list_feature_pypi_dependencies(
        &self,
        feature: FeatureName,
        target: Option<&TargetSelector>,
    ) -> Option<HashMap<PypiPackageName, Vec<PixiPypiSpec>>> {
        crate::workspace::workspace::feature::list_feature_pypi_dependencies(
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

    pub async fn feature_by_task(
        &self,
        task: &TaskName,
        environment: &EnvironmentName,
    ) -> Option<FeatureName> {
        crate::workspace::workspace::feature::feature_by_task(&self.workspace, task, environment)
            .await
    }

    pub async fn remove_feature(
        &self,
        feature: &FeatureName,
    ) -> miette::Result<Vec<EnvironmentName>> {
        crate::workspace::workspace::feature::remove_feature(
            &self.interface,
            self.workspace_mut()?,
            feature,
        )
        .await
    }

    pub async fn add_conda_deps(
        &self,
        specs: IndexMap<PackageName, MatchSpec>,
        spec_type: SpecType,
        dep_options: DependencyOptions,
        git_options: GitOptions,
    ) -> miette::Result<Option<UpdateDeps>> {
        Box::pin(crate::workspace::add::add_conda_dep(
            self.workspace_mut()?,
            specs,
            spec_type,
            dep_options,
            git_options,
        ))
        .await
    }

    pub async fn add_pypi_deps(
        &self,
        pypi_deps: PypiDeps,
        editable: bool,
        options: DependencyOptions,
    ) -> miette::Result<Option<UpdateDeps>> {
        Box::pin(crate::workspace::add::add_pypi_dep(
            self.workspace_mut()?,
            pypi_deps,
            editable,
            options,
        ))
        .await
    }

    pub async fn remove_conda_deps(
        &self,
        specs: IndexMap<PackageName, MatchSpec>,
        spec_type: SpecType,
        dep_options: DependencyOptions,
    ) -> miette::Result<()> {
        Box::pin(crate::workspace::remove::remove_conda_deps(
            self.workspace_mut()?,
            specs,
            spec_type,
            dep_options,
        ))
        .await
    }

    pub async fn remove_pypi_deps(
        &self,
        pypi_deps: PypiDeps,
        options: DependencyOptions,
    ) -> miette::Result<()> {
        Box::pin(crate::workspace::remove::remove_pypi_deps(
            self.workspace_mut()?,
            pypi_deps,
            options,
        ))
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

    pub async fn list_tasks(
        &self,
        environment: Option<EnvironmentName>,
    ) -> miette::Result<HashMap<EnvironmentName, HashMap<TaskName, Task>>> {
        crate::workspace::task::list_tasks(&self.workspace, environment).await
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

    pub async fn search_exact(
        &self,
        match_spec: MatchSpec,
        channels: IndexSet<Channel>,
        platform: Platform,
    ) -> miette::Result<Option<Vec<RepoDataRecord>>> {
        crate::workspace::search::search_exact(
            Some(&self.workspace),
            match_spec,
            channels,
            platform,
        )
        .await
    }

    /// Returns all matching packages with their latest versions
    pub async fn search_wildcard(
        &self,
        search: &str,
        channels: IndexSet<Channel>,
        platform: Platform,
    ) -> miette::Result<Option<Vec<RepoDataRecord>>> {
        crate::workspace::search::search_wildcard(Some(&self.workspace), search, channels, platform)
            .await
    }
}
