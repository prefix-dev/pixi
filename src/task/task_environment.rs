use crate::project::Environment;
use crate::task::error::{AmbiguousTaskError, MissingTaskError};
use crate::{Project, Task};
use itertools::Itertools;
use miette::Diagnostic;
use rattler_conda_types::Platform;
use thiserror::Error;

/// Defines where the task was defined when looking for a task.
#[derive(Debug, Clone)]
pub enum FindTaskSource<'p> {
    CmdArgs,
    DependsOn(String, &'p Task),
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
    pub task_name: String,
    pub depended_on_by: Option<(String, &'p Task)>,
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
        name: &str,
        source: FindTaskSource<'p>,
    ) -> Result<TaskAndEnvironment<'p>, FindTaskError> {
        // If the task was specified on the command line and there is no explicit environment and
        // the task is only defined in the default feature, use the default environment.
        if matches!(source, FindTaskSource::CmdArgs) && self.explicit_environment.is_none() {
            if let Some(task) = self
                .project
                .manifest
                .default_feature()
                .targets
                .resolve(self.platform)
                .find_map(|target| target.tasks.get(name))
            {
                // None of the other environments can have this task. Otherwise, its still
                // ambiguous.
                if !self
                    .project
                    .environments()
                    .into_iter()
                    .flat_map(|env| env.features(false).collect_vec())
                    .flat_map(|feature| feature.targets.resolve(self.platform))
                    .any(|target| target.tasks.contains_key(name))
                {
                    return Ok((self.project.default_environment(), task));
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
        let include_default_feature = true;
        let mut tasks = Vec::new();
        for env in environments.iter() {
            if let Some(task) = env
                .tasks(self.platform, include_default_feature)
                .ok()
                .and_then(|tasks| tasks.get(name).copied())
            {
                tasks.push((env.clone(), task));
            }
        }

        match tasks.len() {
            0 => Err(FindTaskError::MissingTask(MissingTaskError {
                task_name: name.to_string(),
            })),
            1 => {
                let (env, task) = tasks.remove(0);
                Ok((env.clone(), task))
            }
            _ => {
                let ambiguous_task = AmbiguousTask {
                    task_name: name.to_string(),
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
