use crate::project::virtual_packages::verify_current_platform_has_required_virtual_packages;
use crate::project::Environment;
use crate::task::error::{AmbiguousTaskError, MissingTaskError};
use crate::task::TaskName;
use crate::{Project, Task};
use miette::Diagnostic;
use rattler_conda_types::Platform;
use thiserror::Error;

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
    pub project: &'p Project,
    pub explicit_environment: Option<Environment<'p>>,
    pub platform: Option<Platform>,
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
    // If the user specified an environment, look for tasks in the main environment and the
    // user specified environment.
    //
    // If the user did not specify an environment, look for tasks in any environment.
    pub fn from_opt_env(
        project: &'p Project,
        explicit_environment: Option<Environment<'p>>,
        platform: Option<Platform>,
    ) -> Self {
        Self {
            project,
            explicit_environment,
            platform,
            disambiguate: NoDisambiguation,
        }
    }
}

impl<'p, D: TaskDisambiguation<'p>> SearchEnvironments<'p, D> {
    /// Returns a new `SearchEnvironments` with the given disambiguation function.
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

    /// Finds the task with the given name or returns an error that explains why the task could not
    /// be found.
    pub fn find_task(
        &self,
        name: TaskName,
        source: FindTaskSource<'p>,
    ) -> Result<TaskAndEnvironment<'p>, FindTaskError> {
        // If no explicit environment was specified
        if self.explicit_environment.is_none() {
            let default_env = self.project.default_environment();
            // If the default environment has the task
            if let Ok(default_env_task) = default_env.task(&name, self.platform) {
                // If no other environment has the task name but a different task, return the default environment
                if !self
                    .project
                    .environments()
                    .iter()
                    // Filter out default environment
                    .filter(|env| !env.name().is_default())
                    // Filter out environments that can not run on this machine.
                    .filter(|env| {
                        verify_current_platform_has_required_virtual_packages(env).is_ok()
                    })
                    .any(|env| {
                        if let Ok(task) = env.task(&name, self.platform) {
                            // If the task exists in the environment but it is not the reference to the same task, return true to make it ambiguous
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

        // If an explicit environment was specified, only look for tasks in that environment and
        // the default environment.
        let environments = if let Some(explicit_environment) = &self.explicit_environment {
            vec![explicit_environment.clone()]
        } else {
            self.project.environments()
        };

        // Find all the task and environment combinations
        let mut tasks = Vec::new();
        for env in environments.iter() {
            if let Some(task) = env
                .tasks(self.platform)
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
    use super::*;
    use std::path::Path;

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
        let project = Project::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let env = project.default_environment();
        let search = SearchEnvironments::from_opt_env(&project, None, Some(env.best_platform()));
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs);
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
        let project = Project::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs);
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
        "#;
        let project = Project::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs);
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));

        // With explicit environment
        let search =
            SearchEnvironments::from_opt_env(&project, Some(project.default_environment()), None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs);
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
        "#;
        let project = Project::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs);
        assert!(result.unwrap().0.name().is_default());

        // With explicit environment
        let search = SearchEnvironments::from_opt_env(
            &project,
            Some(project.environment("prod").unwrap()),
            None,
        );
        let result = search.find_task("test".into(), FindTaskSource::CmdArgs);
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
        let project = Project::from_str(Path::new("pixi.toml"), manifest_str).unwrap();
        let search = SearchEnvironments::from_opt_env(&project, None, None);
        let result = search.find_task("bla".into(), FindTaskSource::CmdArgs);
        // Ambiguous task because it is the same name and code but it is defined in different environments
        assert!(matches!(result, Err(FindTaskError::AmbiguousTask(_))));
    }
}
