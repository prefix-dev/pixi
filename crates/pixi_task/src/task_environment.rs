use miette::Diagnostic;
use pixi_core::{
    Workspace,
    workspace::{Environment, virtual_packages::verify_current_platform_can_run_environment},
};
use pixi_manifest::{FeaturesExt, HasWorkspaceManifest, PixiPlatform, Task, TaskName};
use thiserror::Error;

use crate::error::{AmbiguousTaskError, MissingTaskError};

/// Defines where the task was defined when looking for a task.
#[derive(Debug, Clone)]
pub enum FindTaskSource<'p> {
    CmdArgs,
    DependsOn(TaskName, &'p Task),
}

pub type TaskAndEnvironment<'p> = (Environment<'p>, &'p Task);

pub trait TaskDisambiguation<'p> {
    fn disambiguate(&self, task: &AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>>;
}

#[derive(Default)]
pub struct NoDisambiguation;
pub struct DisambiguateFn<Fn>(Fn);

impl<'p> TaskDisambiguation<'p> for NoDisambiguation {
    fn disambiguate(&self, _task: &AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>> {
        None
    }
}

impl<'p, F: Fn(&AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>>> TaskDisambiguation<'p>
    for DisambiguateFn<F>
{
    fn disambiguate(&self, task: &AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>> {
        self.0(task)
    }
}

/// An object to help with searching for tasks.
pub struct SearchEnvironments<'p, D: TaskDisambiguation<'p> = NoDisambiguation> {
    pub project: &'p Workspace,
    pub explicit_environment: Option<Environment<'p>>,
    pub platform: Option<&'p PixiPlatform>,
    pub disambiguate: D,
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
}

impl<'p> SearchEnvironments<'p, NoDisambiguation> {
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
        platform: Option<&'p PixiPlatform>,
    ) -> Self {
        Self {
            project,
            explicit_environment,
            platform,
            disambiguate: NoDisambiguation,
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

/// The platform to resolve an environment's task targets against when the
/// caller did not pin one: the platform the environment was last installed
/// for, the best declared platform for this machine, or -- so tasks of
/// machine-incompatible environments are still found -- the first declared
/// platform.
fn default_search_platform<'p>(env: &Environment<'p>) -> Option<&'p PixiPlatform> {
    if let Some(installed) = env.installed_resolved_platform_name()
        && let Some(platform) = env.named_or_best_declared_platform(Some(&installed))
    {
        return Some(platform);
    }
    env.best_declared_platform().or_else(|| {
        let env_platform_names = env.platforms();
        env.workspace_manifest()
            .workspace
            .platforms
            .iter()
            .find(|platform| env_platform_names.contains(platform.name()))
    })
}

impl<'p, D: TaskDisambiguation<'p>> SearchEnvironments<'p, D> {
    /// Returns a new `SearchEnvironments` with the given disambiguation
    /// function.
    pub fn with_disambiguate_fn<F: Fn(&AmbiguousTask<'p>) -> Option<TaskAndEnvironment<'p>>>(
        self,
        func: F,
    ) -> SearchEnvironments<'p, DisambiguateFn<F>> {
        SearchEnvironments {
            project: self.project,
            explicit_environment: self.explicit_environment,
            platform: self.platform,
            disambiguate: DisambiguateFn(func),
        }
    }

    /// The platform to resolve `env`'s task targets against: the caller's
    /// pinned platform when one was given, otherwise the environment's own
    /// default (installed / best declared / first declared).
    fn search_platform_for(&self, env: &Environment<'p>) -> Option<&'p PixiPlatform> {
        match self.platform {
            Some(platform) => Some(platform),
            None => default_search_platform(env),
        }
    }

    /// Finds the task with the given name or returns an error that explains why
    /// the task could not be found.
    pub(crate) fn find_task(
        &self,
        name: TaskName,
        source: FindTaskSource<'p>,
        task_specific_environment: Option<Environment<'p>>,
    ) -> Result<TaskAndEnvironment<'p>, FindTaskError> {
        // If no explicit environment was specified
        if self.explicit_environment.is_none() && task_specific_environment.is_none() {
            let default_env = self.project.default_environment();
            // If the default environment has the task
            if let Ok(default_env_task) =
                default_env.task(&name, self.search_platform_for(&default_env))
            {
                // If the task in the default environment declares a `default-environment`
                // and that environment exists and can run on this platform, prefer that
                // environment instead of returning the default environment.
                if let Some(default_env_name) = default_env_task.default_environment()
                    && let Some(env) = self
                        .project
                        .environments()
                        .into_iter()
                        .find(|e| e.name() == default_env_name)
                    && verify_current_platform_can_run_environment(&env, None).is_ok()
                    && let Ok(task_in_env) = env.task(&name, self.search_platform_for(&env))
                {
                    return Ok((env.clone(), task_in_env));
                }
                // If no other environment has the task name but a different task, return the
                // default environment
                if !self
                    .project
                    .environments()
                    .iter()
                    // Filter out default environment
                    .filter(|env| !env.name().is_default())
                    // Filter out environments that can not run on this machine.
                    .filter(|env| verify_current_platform_can_run_environment(env, None).is_ok())
                    .any(|env| {
                        if let Ok(task) = env.task(&name, self.search_platform_for(env)) {
                            // If the task exists in the environment but it is not the reference to
                            // the same task, return true to make it ambiguous
                            !std::ptr::eq(task, default_env_task)
                        } else {
                            // If the task does not exist in the environment, return false
                            false
                        }
                    })
                {
                    return Ok((self.project.default_environment(), default_env_task));
                }
            }
        }

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
        for env in environments.iter() {
            if let Some(task) = env
                .tasks(self.search_platform_for(env))
                .ok()
                .and_then(|tasks| tasks.get(&name).copied())
            {
                tasks.push((env.clone(), task));
            }
        }

        match tasks.len() {
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

                match self.disambiguate.disambiguate(&ambiguous_task) {
                    Some(env) => Ok(env),
                    None => Err(FindTaskError::AmbiguousTask(ambiguous_task.into())),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A task that only exists for an environment whose platforms exclude
    /// this machine must still be found: each environment resolves its own
    /// search platform (falling back to its first declared platform), rather
    /// than inheriting the caller's.
    #[test]
    fn test_find_task_in_foreign_platform_environment() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let (env, _task) = search
            .find_task("flash".into(), FindTaskSource::CmdArgs, None)
            .expect("task in a foreign-platform environment should be found");
        assert_eq!(env.name().as_str(), "riscv");
    }

    #[test]
    fn test_find_task_default_defined() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, env.best_declared_platform());
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(result.is_ok());
        assert!(result.unwrap().0.name().is_default());
    }

    #[test]
    fn test_find_task_dual_defined() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));
    }

    #[test]
    fn test_find_task_explicit_defined() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));

        // With explicit environment
        let search =
            SearchEnvironments::from_opt_env(&project, Some(project.default_environment()), None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(result.unwrap().0.name().is_default());
    }

    #[test]
    fn test_find_non_default_feature_task() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(result.unwrap().0.name().is_default());

        // With explicit environment
        let search = SearchEnvironments::from_opt_env(
            &project,
            Some(project.environment("prod").unwrap()),
            None,
        );
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs, None);
        assert!(matches!(result, Err(FindTaskError::MissingTask(_))));
    }

    #[test]
    fn test_find_ambiguous_task() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("bla".into(), FindTaskSource::CmdArgs, None);
        // Ambiguous task because it is the same name and code but it is defined in
        // different environments
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));
    }

    #[test]
    fn test_default_environment_preferred_when_multiple_envs() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, None).with_disambiguate_fn(
            |amb: &AmbiguousTask| {
                amb.environments
                    .iter()
                    .find(|(env, task)| {
                        if let Some(default_env_name) = task.default_environment() {
                            default_env_name == env.name()
                        } else {
                            false
                        }
                    })
                    .cloned()
            },
        );

        // When resolving `test3` we expect the it to pick `three`.
        let result = search
            .find_task("test3".into(), FindTaskSource::CmdArgs, None)
            .expect("should pick default environment");
        assert_eq!(result.0.name().as_str(), "three");
    }

    #[test]
    fn test_explicit_environment_overrides_task_default_environment() {
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
        let search = SearchEnvironments::from_opt_env(&project, Some(explicit_env), None);
        let result = search.find_task("test3".into(), FindTaskSource::CmdArgs, None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().0.name().as_str(), "two");
    }

    #[test]
    fn test_top_level_task_default_environment_is_used() {
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
        let search = SearchEnvironments::from_opt_env(&project, None, None);

        // Resolve `test` task; since the task declares `default-environment = "test"`
        // we expect the resolved environment to be `test` rather than the default.
        let result = search
            .find_task("test".into(), FindTaskSource::CmdArgs, None)
            .expect("should resolve to an environment");
        assert_eq!(result.0.name().as_str(), "test");
    }
}
