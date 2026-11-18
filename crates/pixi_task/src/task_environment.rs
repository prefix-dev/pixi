use std::{collections::HashMap, future::Future, sync::Arc};

use miette::Diagnostic;
use pixi_core::{
    Workspace,
    lock_file::UpdatePrefixError,
    workspace::{
        Environment,
        platform_selection::{PlatformResolutionError, ResolvedPlatform},
        virtual_packages::{EnvironmentRunnability, classify_resolved_platform_runnability},
    },
};
use pixi_manifest::{
    EnvironmentName, FeaturesExt, HasWorkspaceManifest, PixiPlatform, PixiPlatformName, Task,
    TaskName,
};
use thiserror::Error;

use crate::error::{AmbiguousTaskError, MissingTaskError};

/// Defines where the task was defined when looking for a task.
#[derive(Debug, Clone)]
pub enum FindTaskSource<'p> {
    CmdArgs,
    DependsOn(TaskName, &'p Task),
}

pub type TaskAndEnvironment<'p> = (Environment<'p>, &'p Task);

/// Why an ambiguous task couldn't be disambiguated, e.g. a failed prompt.
pub type DisambiguationSource = Box<dyn std::error::Error + Send + Sync>;

/// Picks the environment to run an ambiguous task in. `Ok(None)` leaves the
/// task ambiguous.
pub trait TaskDisambiguation<'p> {
    fn disambiguate(
        &self,
        task: &AmbiguousTask<'p>,
    ) -> impl Future<Output = Result<Option<TaskAndEnvironment<'p>>, DisambiguationSource>>;
}

#[derive(Default)]
pub struct NoDisambiguation;
pub struct DisambiguateFn<Fn>(Fn);

impl<'p> TaskDisambiguation<'p> for NoDisambiguation {
    async fn disambiguate(
        &self,
        _task: &AmbiguousTask<'p>,
    ) -> Result<Option<TaskAndEnvironment<'p>>, DisambiguationSource> {
        Ok(None)
    }
}

impl<
    'p,
    F: AsyncFn(&AmbiguousTask<'p>) -> Result<Option<TaskAndEnvironment<'p>>, DisambiguationSource>,
> TaskDisambiguation<'p> for DisambiguateFn<F>
{
    async fn disambiguate(
        &self,
        task: &AmbiguousTask<'p>,
    ) -> Result<Option<TaskAndEnvironment<'p>>, DisambiguationSource> {
        (self.0)(task).await
    }
}

/// An object to help with searching for tasks.
pub struct SearchEnvironments<'p, F, D: TaskDisambiguation<'p> = NoDisambiguation> {
    pub project: &'p Workspace,
    pub explicit_environment: Option<Environment<'p>>,
    pub platform: Option<PixiPlatformName>,
    pub disambiguate: D,
    resolve_platform: F,
    /// Each environment's platform resolution, made at most once.
    resolutions: HashMap<EnvironmentName, Result<ResolvedPlatform<'p>, Arc<UpdatePrefixError>>>,
    installed_ambiguity: HashMap<EnvironmentName, bool>,
}

/// Information about an task that was found when searching for a task
pub struct AmbiguousTask<'p> {
    pub task_name: TaskName,
    pub depended_on_by: Option<(TaskName, &'p Task)>,
    pub environments: Vec<TaskAndEnvironment<'p>>,
}

impl<'p> From<AmbiguousTask<'p>> for AmbiguousTaskError {
    fn from(value: AmbiguousTask<'p>) -> Self {
        Self {
            task_name: value.task_name,
            environments: value
                .environments
                .into_iter()
                .map(|env| env.0.name().clone())
                .collect(),
        }
    }
}

#[derive(Debug, Diagnostic, Error)]
pub enum FindTaskError {
    #[error(transparent)]
    MissingTask(MissingTaskError),

    #[error(transparent)]
    AmbiguousTask(AmbiguousTaskError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Disambiguation(DisambiguationError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PlatformResolution(#[from] Arc<UpdatePrefixError>),
}

/// Choosing the environment to run an ambiguous task in failed.
#[derive(Debug, Diagnostic, Error)]
#[error("failed to choose the environment to run the task '{}' in", .ambiguous.task_name)]
#[diagnostic(forward(ambiguous))]
pub struct DisambiguationError {
    /// The task that is left ambiguous, whose help explains how to choose
    /// the environment instead.
    pub ambiguous: AmbiguousTaskError,
    #[source]
    pub source: DisambiguationSource,
}

impl<'p, F> SearchEnvironments<'p, F, NoDisambiguation>
where
    F: AsyncFn(&Environment<'p>) -> Result<ResolvedPlatform<'p>, UpdatePrefixError>,
{
    // Determine which environments we are allowed to check for tasks.
    //
    // If the user specified an environment, look for tasks in the main environment
    // and the user specified environment.
    //
    // If the user did not specify an environment, look for tasks in any
    // environment.
    pub fn from_opt_env(
        project: &'p Workspace,
        explicit_environment: Option<Environment<'p>>,
        platform: Option<PixiPlatformName>,
        resolve_platform: F,
    ) -> Self {
        Self {
            project,
            explicit_environment,
            platform,
            disambiguate: NoDisambiguation,
            resolve_platform,
            resolutions: HashMap::new(),
            installed_ambiguity: HashMap::new(),
        }
    }
}

/// Environments that define `name` for any of their declared platforms or
/// in a platform-independent target, regardless of whether this machine can
/// run them.
pub(crate) fn environments_defining_task(
    project: &Workspace,
    name: &TaskName,
) -> Vec<pixi_manifest::EnvironmentName> {
    project
        .environments()
        .into_iter()
        .filter(|env| {
            let env_platform_names = env.platforms();
            let declared = env
                .workspace_manifest()
                .workspace
                .platforms
                .iter()
                .filter(|platform| env_platform_names.contains(platform.name()));
            std::iter::once(None)
                .chain(declared.map(Some))
                .any(|platform| env.task(name, platform).is_ok())
        })
        .map(|env| env.name().clone())
        .collect()
}

impl<'p, F, D> SearchEnvironments<'p, F, D>
where
    F: AsyncFn(&Environment<'p>) -> Result<ResolvedPlatform<'p>, UpdatePrefixError>,
    D: TaskDisambiguation<'p>,
{
    /// Returns a new `SearchEnvironments` with the given disambiguation
    /// function.
    pub fn with_disambiguate_fn<
        G: AsyncFn(&AmbiguousTask<'p>) -> Result<Option<TaskAndEnvironment<'p>>, DisambiguationSource>,
    >(
        self,
        func: G,
    ) -> SearchEnvironments<'p, F, DisambiguateFn<G>> {
        SearchEnvironments {
            project: self.project,
            explicit_environment: self.explicit_environment,
            platform: self.platform,
            disambiguate: DisambiguateFn(func),
            resolve_platform: self.resolve_platform,
            resolutions: self.resolutions,
            installed_ambiguity: self.installed_ambiguity,
        }
    }

    fn resolved_platform(&self, env: &Environment<'p>) -> Option<&ResolvedPlatform<'p>> {
        self.resolutions.get(env.name())?.as_ref().ok()
    }

    pub fn search_platform_for(&self, env: &Environment<'p>) -> Option<&PixiPlatform> {
        self.resolved_platform(env).map(ResolvedPlatform::platform)
    }

    /// The platforms of the environments that resolved successfully.
    pub fn into_platforms(self) -> HashMap<EnvironmentName, ResolvedPlatform<'p>> {
        self.resolutions
            .into_iter()
            .filter_map(|(name, resolution)| Some((name, resolution.ok()?)))
            .collect()
    }

    pub(crate) async fn prepare_environment(
        &mut self,
        env: &Environment<'p>,
    ) -> Result<(), Arc<UpdatePrefixError>> {
        if !self.resolutions.contains_key(env.name()) {
            let resolution = (self.resolve_platform)(env).await.map_err(Arc::new);
            self.resolutions.insert(env.name().clone(), resolution);
        }
        match &self.resolutions[env.name()] {
            Ok(_) => Ok(()),
            Err(error) => Err(Arc::clone(error)),
        }
    }

    /// Like [`Self::prepare_environment`], but `Ok(false)` when `env` cannot
    /// run on this machine, so the caller can look for the task elsewhere.
    /// Other failures are errors: skipping them would silently run the task in
    /// a different environment.
    async fn prepare_if_runnable(&mut self, env: &Environment<'p>) -> Result<bool, FindTaskError> {
        match self.prepare_environment(env).await {
            Ok(()) => Ok(true),
            Err(error)
                if matches!(
                    *error,
                    UpdatePrefixError::PlatformResolution(
                        PlatformResolutionError::UnsupportedPlatform(_)
                            | PlatformResolutionError::RequirementsUnmet(_)
                    )
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn can_disambiguate(&mut self, environment: &Environment<'p>) -> bool {
        let selection = self
            .resolutions
            .get(environment.name())
            .and_then(|resolution| resolution.as_ref().ok());
        match selection {
            Some(
                ResolvedPlatform::Explicit(_)
                | ResolvedPlatform::HostMatch(_)
                | ResolvedPlatform::HostBaseline(_),
            ) => true,
            Some(ResolvedPlatform::Installed(_)) => *self
                .installed_ambiguity
                .entry(environment.name().clone())
                .or_insert_with(|| {
                    matches!(
                        classify_resolved_platform_runnability(environment, selection),
                        EnvironmentRunnability::ByDesign | EnvironmentRunnability::NoDependencies
                    )
                }),
            _ => false,
        }
    }

    /// Check definitions before probing: an absent task or a sibling that only
    /// inherits the default task cannot affect task disambiguation.
    fn has_task_candidate(
        &self,
        env: &Environment<'p>,
        name: &TaskName,
        inherited: Option<&Task>,
    ) -> bool {
        let is_candidate = |platform| {
            env.task(name, platform)
                .is_ok_and(|task| inherited.is_none_or(|inherited| !std::ptr::eq(task, inherited)))
        };
        let platforms = env.platforms();
        std::iter::once(None)
            .chain(
                env.workspace_manifest()
                    .workspace
                    .platforms
                    .iter()
                    .filter(|platform| platforms.contains(platform.name()))
                    .map(Some),
            )
            .any(is_candidate)
    }

    /// Finds the task with the given name or returns an error that explains why
    /// the task could not be found.
    pub(crate) async fn find_task(
        &mut self,
        name: TaskName,
        source: FindTaskSource<'p>,
        task_specific_environment: Option<Environment<'p>>,
    ) -> Result<TaskAndEnvironment<'p>, FindTaskError> {
        // If no explicit environment was specified
        if self.explicit_environment.is_none() && task_specific_environment.is_none() {
            let default_env = self.project.default_environment();
            let default_env_task = if self.has_task_candidate(&default_env, &name, None)
                && self.prepare_if_runnable(&default_env).await?
            {
                default_env
                    .task(&name, self.search_platform_for(&default_env))
                    .ok()
            } else {
                None
            };
            if let Some(default_env_task) = default_env_task {
                // If the task in the default environment declares a `default-environment`
                // and that environment exists and can run on this platform, prefer that
                // environment instead of returning the default environment.
                if let Some(default_env_name) = default_env_task.default_environment()
                    && let Some(env) = self
                        .project
                        .environments()
                        .into_iter()
                        .find(|e| e.name() == default_env_name)
                    && self.has_task_candidate(&env, &name, None)
                    && self.prepare_if_runnable(&env).await?
                    && let Ok(task_in_env) = env.task(&name, self.search_platform_for(&env))
                {
                    return Ok((env, task_in_env));
                }
                // If no other environment has the task name but a different task, return the
                // default environment
                let mut ambiguous = false;
                for env in self.project.environments() {
                    if env.name().is_default()
                        || !self.has_task_candidate(&env, &name, Some(default_env_task))
                    {
                        continue;
                    }
                    // Lock-only fallbacks and incompatible siblings do not make a default task ambiguous.
                    if self.prepare_environment(&env).await.is_ok()
                        && self.can_disambiguate(&env)
                        && let Ok(task) = env.task(&name, self.search_platform_for(&env))
                        && !std::ptr::eq(task, default_env_task)
                    {
                        ambiguous = true;
                        break;
                    }
                }
                if !ambiguous {
                    return Ok((self.project.default_environment(), default_env_task));
                }
            }
        }
        let required_environment =
            task_specific_environment.is_some() || self.explicit_environment.is_some();

        let environments = match (task_specific_environment, &self.explicit_environment) {
            (Some(task_specific_environment), _) => {
                // If a specific environment was specified in the dependency, only look for tasks in that
                // environment.
                vec![task_specific_environment]
            }
            (None, Some(explicit_environment)) => {
                // If an explicit environment was specified, only look for tasks in that
                // environment and the default environment.
                Vec::from([explicit_environment.clone()])
            }
            _ => {
                // If no specific environment was specified, look for tasks in all environments.
                self.project.environments()
            }
        };

        // Find all the task and environment combinations
        let mut tasks = Vec::new();
        let mut first_error = None;
        for env in environments.iter() {
            if !self.has_task_candidate(env, &name, None) {
                continue;
            }
            if let Err(error) = self.prepare_environment(env).await {
                if required_environment {
                    return Err(error.into());
                }
                first_error.get_or_insert(error);
                continue;
            }
            if let Some(task) = env
                .tasks(self.search_platform_for(env))
                .ok()
                .and_then(|tasks| tasks.get(&name).copied())
            {
                tasks.push((env.clone(), task));
            }
        }

        match tasks.len() {
            0 if first_error.is_some() => Err(first_error.unwrap().into()),
            0 => Err(FindTaskError::MissingTask(MissingTaskError {
                task_name: name,
            })),
            1 => {
                let (env, task) = tasks.remove(0);
                Ok((env.clone(), task))
            }
            _ => {
                let ambiguous_task = AmbiguousTask {
                    task_name: name,
                    depended_on_by: match source {
                        FindTaskSource::DependsOn(dep, task) => Some((dep, task)),
                        _ => None,
                    },
                    environments: tasks,
                };

                match self.disambiguate.disambiguate(&ambiguous_task).await {
                    Ok(Some(env)) => Ok(env),
                    Ok(None) => Err(FindTaskError::AmbiguousTask(ambiguous_task.into())),
                    Err(source) => Err(FindTaskError::Disambiguation(DisambiguationError {
                        ambiguous: ambiguous_task.into(),
                        source,
                    })),
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn search_for_test<'p>(
    project: &'p Workspace,
    environment: Option<Environment<'p>>,
    platform: Option<&'p PixiPlatform>,
) -> SearchEnvironments<
    'p,
    impl AsyncFn(&Environment<'p>) -> Result<ResolvedPlatform<'p>, UpdatePrefixError>,
> {
    SearchEnvironments::from_opt_env(
        project,
        environment,
        platform.map(|platform| platform.name().clone()),
        async move |env: &Environment<'p>| {
            let selected = platform
                .or_else(|| env.best_declared_platform())
                .or_else(|| {
                    let platforms = env.platforms();
                    project
                        .workspace_manifest()
                        .workspace
                        .platforms
                        .iter()
                        .find(|platform| platforms.contains(platform.name()))
                });
            Ok(match selected {
                Some(platform) => ResolvedPlatform::HostMatch(platform),
                None => ResolvedPlatform::HostBaseline(PixiPlatform::from_subdir(
                    rattler_conda_types::Subdir::current()
                        .unwrap_or(rattler_conda_types::Subdir::NoArch),
                )),
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// Task lookup uses the caller's explicit per-environment selection even
    /// when that environment's task targets differ from the host.
    #[tokio::test]
    async fn test_find_task_in_foreign_platform_environment() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64", "linux-riscv64"]

            [feature.riscv]
            platforms = ["linux-riscv64"]

            [feature.riscv.target.linux-riscv64.tasks]
            flash = "echo flash"

            [environments]
            riscv = ["riscv"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let mut search = search_for_test(&project, None, None);
        let (env, _task) = search
            .find_task("flash".into(), FindTaskSource::CmdArgs, None)
            .await
            .expect("task in a foreign-platform environment should be found");
        assert_eq!(env.name().as_str(), "riscv");
    }

    #[tokio::test]
    async fn test_find_task_default_defined() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "win-64", "osx-64"]

            [tasks]
            test = "cargo test"
            [feature.test.dependencies]
            pytest = "*"
            [environments]
            test = ["test"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let env = project.default_environment();
        let mut search = search_for_test(&project, None, env.best_declared_platform());
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().0.name().is_default());
    }

    #[tokio::test]
    async fn test_find_task_dual_defined() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64", "linux-riscv64"]

            [tasks]
            test = "cargo test"

            [feature.test.tasks]
            test = "cargo test --all-features"

            [environments]
            test = ["test"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let mut search = search_for_test(&project, None, None);
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));
    }

    #[tokio::test]
    async fn test_find_task_explicit_defined() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64", "linux-riscv64"]

            [tasks]
            test = "pytest"
            [feature.test.tasks]
            test = "pytest -s"
            [feature.prod.tasks]
            run = "python start.py"

            [environments]
            default = ["test"]
            test = ["test"]
            prod = ["prod"]

            [system-requirements]
            macos = "10.6"
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let mut search = search_for_test(&project, None, None);
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));

        // With explicit environment
        let mut search = search_for_test(&project, Some(project.default_environment()), None);
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(result.unwrap().0.name().is_default());
    }

    #[tokio::test]
    async fn test_failed_disambiguation_is_reported() {
        let manifest_str = r#"
            [workspace]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64"]

            [feature.a.tasks]
            hi = "echo a"
            [feature.b.tasks]
            hi = "echo b"

            [environments]
            a = ["a"]
            b = ["b"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let mut search = search_for_test(&project, None, None).with_disambiguate_fn(
            async |_: &AmbiguousTask| Err(std::io::Error::other("no answer").into()),
        );
        let result = search
            .find_task("hi".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(matches!(result, Err(FindTaskError::Disambiguation(_))));
    }

    /// A default environment that cannot run on this machine yields the task
    /// to another environment, but any other failure to resolve it is reported
    /// instead of silently running the task elsewhere.
    #[tokio::test]
    async fn test_default_environment_failure_is_only_skipped_when_unrunnable() {
        let manifest_str = r#"
            [workspace]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64"]

            [tasks]
            test = "echo default"
            [feature.other.tasks]
            test = "echo other"

            [environments]
            other = ["other"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        for unrunnable in [true, false] {
            let mut search = SearchEnvironments::from_opt_env(
                &project,
                None,
                None,
                async |env: &Environment<'_>| {
                    if !env.name().is_default() {
                        return Ok(ResolvedPlatform::HostMatch(
                            &env.workspace_manifest().workspace.platforms[0],
                        ));
                    }
                    Err(if unrunnable {
                        PlatformResolutionError::UnsupportedPlatform(Box::new(
                            env.unsupported_platform_error(),
                        ))
                        .into()
                    } else {
                        UpdatePrefixError::LockFileMissingEnv(env.name().clone())
                    })
                },
            );
            let result = search
                .find_task("test".into(), FindTaskSource::CmdArgs, None)
                .await;
            if unrunnable {
                assert_eq!(result.unwrap().0.name().as_str(), "other");
            } else {
                assert!(matches!(
                    result.map(|(env, _)| env.name().clone()),
                    Err(FindTaskError::PlatformResolution(error))
                        if matches!(*error, UpdatePrefixError::LockFileMissingEnv(_))
                ));
            }
        }
    }

    #[tokio::test]
    async fn test_find_non_default_feature_task() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64"]

            [tasks]

            [feature.test.tasks]
            test = "pytest -s"
            [feature.prod.tasks]
            run = "python start.py"

            [environments]
            default = ["test"]
            test = ["test"]
            prod = ["prod"]

            [system-requirements]
            macos = "10.6"
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let mut search = search_for_test(&project, None, None);
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(result.unwrap().0.name().is_default());

        // With explicit environment
        let mut search =
            search_for_test(&project, Some(project.environment("prod").unwrap()), None);
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(matches!(result, Err(FindTaskError::MissingTask(_))));
    }

    #[tokio::test]
    async fn test_find_ambiguous_task() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = ["foo"]
            platforms = ["linux-64", "osx-arm64", "win-64", "osx-64", "linux-riscv64"]

            [tasks]
            bla = "echo foo"

            [feature.other.tasks]
            bla = "echo foo"

            [environments]
            other = ["other"]
        "#;
        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let mut search = search_for_test(&project, None, None);
        let result = search
            .find_task("bla".into(), FindTaskSource::CmdArgs, None)
            .await;
        // Ambiguous task because it is the same name and code but it is defined in
        // different environments
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));
    }

    #[tokio::test]
    async fn test_default_environment_preferred_when_multiple_envs() {
        let manifest_str = r#"
            [workspace]
            channels = []
            platforms = ["linux-64", "win-64", "osx-64", "osx-arm64", "linux-aarch64"]

            [tasks]
            test = "echo test"
            test2 = "echo test2"
            dep.depends-on = ["test3", "test6"]

            [feature.test.tasks]
            test3 = { cmd = "echo test3", default-environment = "three" }
            test4 = "echo test4"

            [feature.test2.tasks]
            test5 = "echo test5"
            test6 = { cmd = "echo test6", default-environment = "four" }

            [environments]
            one = []
            two = ["test"]
            three = ["test2", "test"]
            four = ["test2"]
            five = ["test", "test2"]
            six = { features = ["test"], no-default-feature = true }
            seven = { features = ["test2"], no-default-feature = true }
        "#;

        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();

        // Build a SearchEnvironments that will prefer a candidate environment
        // whose task declares a `default-environment` matching the env name.
        let mut search = search_for_test(&project, None, None).with_disambiguate_fn(
            async |amb: &AmbiguousTask| {
                Ok(amb
                    .environments
                    .iter()
                    .find(|(env, task)| {
                        if let Some(default_env_name) = task.default_environment() {
                            default_env_name == env.name()
                        } else {
                            false
                        }
                    })
                    .cloned())
            },
        );

        // When resolving `test3` we expect the it to pick `three`.
        let result = search
            .find_task("test3".into(), FindTaskSource::CmdArgs, None)
            .await
            .expect("should pick default environment");
        assert_eq!(result.0.name().as_str(), "three");
    }

    #[tokio::test]
    async fn test_explicit_environment_overrides_task_default_environment() {
        let manifest_str = r#"
            [project]
            name = "foo"
            channels = []
            platforms = ["linux-64", "win-64", "osx-64", "osx-arm64", "linux-aarch64"]

            [feature.test.tasks]
            test3 = { cmd = "echo test3", default-environment = "three" }

            [feature.test2.tasks]
            test3 = "echo other"

            [environments]
            default = ["test"]
            two = ["test2"]
            three = ["test"]
        "#;

        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();

        // If the user explicitly requests `two`, that should be preferred even
        // though the task has a `default-environment` set to `three`.
        let explicit_env = project.environment("two").unwrap();
        let mut search = search_for_test(&project, Some(explicit_env), None);
        let result = search
            .find_task("test3".into(), FindTaskSource::CmdArgs, None)
            .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0.name().as_str(), "two");
    }

    #[tokio::test]
    async fn test_top_level_task_default_environment_is_used() {
        let manifest_str = r#"
            [workspace]
            channels = []
            platforms = ["linux-64", "win-64", "osx-64", "osx-arm64", "linux-aarch64"]

            [tasks]
            test = { cmd = "echo test", default-environment = "test" }

            [feature.test.dependencies]

            [environments]
            test = ["test"]
        "#;

        let project = Workspace::from_str(Path::new("pixi.toml"), manifest_str).unwrap();

        // Build a SearchEnvironments that will apply default behavior.
        let mut search = search_for_test(&project, None, None);

        // Resolve `test` task; since the task declares `default-environment = "test"`
        // we expect the resolved environment to be `test` rather than the default.
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .await
            .expect("should resolve to an environment");
        assert_eq!(result.0.name().as_str(), "test");
    }
}
