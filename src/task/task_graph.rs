use crate::project::Environment;
use crate::task::error::AmbiguousTaskError;
use crate::{
    task::{error::MissingTaskError, CmdArgs, Custom, Task},
    Project,
};
use itertools::Itertools;
use miette::Diagnostic;
use rattler_conda_types::Platform;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    env,
    ops::Index,
};
use thiserror::Error;

/// A task ID is a unique identifier for a [`TaskNode`] in a [`TaskGraph`].
///
/// To get a task from a [`TaskGraph`], you can use the [`TaskId`] as an index.
#[derive(Debug, Clone, Copy, Eq, PartialOrd, PartialEq, Ord, Hash)]
pub struct TaskId(usize);

/// A node in the [`TaskGraph`].
pub struct TaskNode<'p> {
    /// The name of the task or `None` if the task is a custom task.
    pub name: Option<String>,

    /// The environment to run the task in
    pub run_environment: Environment<'p>,

    /// A reference to a project task, or a owned custom task.
    pub task: Cow<'p, Task>,

    /// Additional arguments to pass to the command
    pub additional_args: Vec<String>,

    /// The id's of the task that this task depends on.
    pub dependencies: Vec<TaskId>,
}

impl<'p> TaskNode<'p> {
    /// Returns the full command that should be executed for this task. This includes any
    /// additional arguments that should be passed to the command.
    ///
    /// This function returns `None` if the task does not define a command to execute. This is the
    /// case for alias only commands.
    pub fn full_command(&self) -> Option<String> {
        let mut cmd = self.task.as_single_command()?.to_string();

        if !self.additional_args.is_empty() {
            cmd.push(' ');
            cmd.push_str(&self.additional_args.join(" "));
        }

        Some(cmd)
    }
}

/// A [`TaskGraph`] is a graph of tasks that defines the relationships between different executable
/// tasks.
pub struct TaskGraph<'p> {
    /// The project that this graph references
    project: &'p Project,

    /// The tasks in the graph
    nodes: Vec<TaskNode<'p>>,
}

impl<'p> Index<TaskId> for TaskGraph<'p> {
    type Output = TaskNode<'p>;

    fn index(&self, index: TaskId) -> &Self::Output {
        &self.nodes[index.0]
    }
}

/// Defines where the task was defined when looking for a task.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum FindTaskSource {
    CmdArgs,
    DependsOn,
}

/// An object to help with searching for tasks.
struct SearchEnvironments<'p> {
    pub project: &'p Project,
    pub explicit_environment: Option<Environment<'p>>,
    pub platform: Option<Platform>,
}

#[derive(Debug, Diagnostic, Error)]
pub enum FindTaskError {
    #[error(transparent)]
    MissingTask(MissingTaskError),

    #[error(transparent)]
    AmbiguousTask(AmbiguousTaskError),
}

impl<'p> SearchEnvironments<'p> {
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
        }
    }

    /// Finds the task with the given name or returns an error that explains why the task could not
    /// be found.
    pub fn find_task(
        &self,
        name: &str,
        source: FindTaskSource,
    ) -> Result<(Environment<'p>, &'p Task), FindTaskError> {
        // If the task was specified on the command line and there is no explicit environment and
        // the task is only defined in the default feature, use the default environment.
        if source == FindTaskSource::CmdArgs && self.explicit_environment.is_none() {
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
        for env in environments {
            if let Some(task) = env
                .tasks(self.platform, include_default_feature)
                .ok()
                .and_then(|tasks| tasks.get(name).copied())
            {
                tasks.push((env, task));
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
            _ => Err(FindTaskError::AmbiguousTask(AmbiguousTaskError {
                task_name: name.to_string(),
                environments: tasks
                    .into_iter()
                    .map(|(env, _)| env.name().clone())
                    .collect(),
            })),
        }
    }
}

impl<'p> TaskGraph<'p> {
    pub fn project(&self) -> &'p Project {
        self.project
    }

    /// Constructs a new [`TaskGraph`] from a list of command line arguments.
    pub fn from_cmd_args(
        project: &'p Project,
        args: Vec<String>,
        platform: Option<Platform>,
        environment: Option<Environment<'p>>,
    ) -> Result<Self, TaskGraphError> {
        let mut args = args;

        let search_envs = SearchEnvironments::from_opt_env(project, environment, platform);

        if let Some(name) = args.first() {
            match search_envs.find_task(name, FindTaskSource::CmdArgs) {
                Err(FindTaskError::MissingTask(_)) => {}
                Err(FindTaskError::AmbiguousTask(err)) => {
                    return Err(TaskGraphError::AmbiguousTask(err))
                }
                Ok((task_env, task)) => {
                    // If an explicit environment was specified and the task is from the default
                    // environment use the specified environment instead.
                    let run_env = match search_envs.explicit_environment.clone() {
                        Some(explicit_env) if task_env.is_default() => explicit_env,
                        _ => task_env,
                    };

                    return Self::from_root(
                        project,
                        search_envs,
                        TaskNode {
                            name: Some(args.remove(0)),
                            task: Cow::Borrowed(task),
                            run_environment: run_env,
                            additional_args: args,
                            dependencies: vec![],
                        },
                    );
                }
            }
        }

        // When no task is found, just execute the command verbatim.
        let run_environment = search_envs
            .explicit_environment
            .clone()
            .unwrap_or_else(|| project.default_environment());
        Self::from_root(
            project,
            search_envs,
            TaskNode {
                name: None,
                task: Cow::Owned(
                    Custom {
                        cmd: CmdArgs::from(args),
                        cwd: env::current_dir().ok(),
                    }
                    .into(),
                ),
                run_environment,
                additional_args: vec![],
                dependencies: vec![],
            },
        )
    }

    /// Constructs a new instance of a [`TaskGraph`] from a root task.
    fn from_root(
        project: &'p Project,
        search_environments: SearchEnvironments<'p>,
        root: TaskNode<'p>,
    ) -> Result<Self, TaskGraphError> {
        let mut task_name_to_node: HashMap<String, TaskId> =
            HashMap::from_iter(root.name.clone().into_iter().map(|name| (name, TaskId(0))));
        let mut nodes = vec![root];

        // Iterate over all the nodes in the graph and add them to the graph.
        let mut next_node_to_visit = 0;
        while next_node_to_visit < nodes.len() {
            let dependency_names =
                Vec::from_iter(nodes[next_node_to_visit].task.depends_on().iter().cloned());

            // Iterate over all the dependencies of the node and add them to the graph.
            let mut node_dependencies = Vec::with_capacity(dependency_names.len());
            for dependency in dependency_names {
                // Check if we visited this node before already.
                if let Some(&task_id) = task_name_to_node.get(&dependency) {
                    node_dependencies.push(task_id);
                    continue;
                }

                // Find the task in the project
                let (task_env, task_dependency) =
                    match search_environments.find_task(&dependency, FindTaskSource::DependsOn) {
                        Err(FindTaskError::MissingTask(err)) => {
                            return Err(TaskGraphError::MissingTask(err))
                        }
                        Err(FindTaskError::AmbiguousTask(err)) => {
                            return Err(TaskGraphError::AmbiguousTask(err))
                        }
                        Ok(result) => result,
                    };

                // Add the node to the graph
                let task_id = TaskId(nodes.len());
                nodes.push(TaskNode {
                    name: Some(dependency.clone()),
                    task: Cow::Borrowed(task_dependency),
                    run_environment: task_env,
                    additional_args: Vec::new(),
                    dependencies: Vec::new(),
                });

                // Store the task id in the map to be able to look up the name later
                task_name_to_node.insert(dependency, task_id);

                // Add the dependency to the node
                node_dependencies.push(task_id);
            }

            nodes[next_node_to_visit].dependencies = node_dependencies;
            next_node_to_visit += 1;
        }

        Ok(Self { project, nodes })
    }

    /// Returns the topological order of the tasks in the graph.
    ///
    /// The topological order is the order in which the tasks should be executed to ensure that
    /// all dependencies of a task are executed before the task itself.
    pub fn topological_order(&self) -> Vec<TaskId> {
        let mut visited = HashSet::new();
        let mut order = Vec::new();

        for i in 0..self.nodes.len() {
            visit(TaskId(i), &self.nodes, &mut visited, &mut order);
        }

        return order;

        fn visit(
            id: TaskId,
            nodes: &[TaskNode<'_>],
            visited: &mut HashSet<TaskId>,
            order: &mut Vec<TaskId>,
        ) {
            if !visited.insert(id) {
                return;
            }

            for dependency in nodes[id.0].dependencies.iter() {
                visit(*dependency, nodes, visited, order);
            }

            order.push(id);
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum TaskGraphError {
    #[error(transparent)]
    MissingTask(#[from] MissingTaskError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    AmbiguousTask(AmbiguousTaskError),
}

#[cfg(test)]
mod test {
    use crate::task::task_graph::TaskGraph;
    use crate::{EnvironmentName, Project};
    use rattler_conda_types::Platform;
    use std::path::Path;

    fn commands_in_order(
        project_str: &str,
        run_args: &[&str],
        platform: Option<Platform>,
        environment_name: Option<EnvironmentName>,
    ) -> Vec<String> {
        let project = Project::from_str(Path::new(""), project_str).unwrap();
        let environment = environment_name.map(|name| project.environment(&name).unwrap());
        let graph = TaskGraph::from_cmd_args(
            &project,
            run_args.into_iter().map(|arg| arg.to_string()).collect(),
            platform,
            environment,
        )
        .unwrap();

        graph
            .topological_order()
            .into_iter()
            .map(|task| &graph[task])
            .filter_map(|task| task.full_command())
            .collect()
    }

    #[test]
    fn test_ordered_commands() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
        [tasks]
        root = "echo root"
        task1 = {cmd="echo task1", depends_on=["root"]}
        task2 = {cmd="echo task2", depends_on=["root"]}
        top = {cmd="echo top", depends_on=["task1","task2"]}
    "#,
                &["top", "--test"],
                None,
                None
            ),
            vec!["echo root", "echo task1", "echo task2", "echo top --test"]
        );
    }

    #[test]
    fn test_cycle_ordered_commands() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
        [tasks]
        root = {cmd="echo root", depends_on=["task1"]}
        task1 = {cmd="echo task1", depends_on=["root"]}
        task2 = {cmd="echo task2", depends_on=["root"]}
        top = {cmd="echo top", depends_on=["task1","task2"]}
    "#,
                &["top"],
                None,
                None
            ),
            vec!["echo root", "echo task1", "echo task2", "echo top"]
        );
    }

    #[test]
    fn test_platform_ordered_commands() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
        [tasks]
        root = "echo root"
        task1 = {cmd="echo task1", depends_on=["root"]}
        task2 = {cmd="echo task2", depends_on=["root"]}
        top = {cmd="echo top", depends_on=["task1","task2"]}
        [target.linux-64.tasks]
        root = {cmd="echo linux", depends_on=["task1"]}
    "#,
                &["top"],
                Some(Platform::Linux64),
                None
            ),
            vec!["echo linux", "echo task1", "echo task2", "echo top",]
        );
    }

    #[test]
    fn test_custom_command() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
    "#,
                &["echo bla"],
                None,
                None
            ),
            vec![r#""echo bla""#]
        );
    }

    #[test]
    fn test_multi_env() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]

        [feature.build.tasks]
        build = "echo build"

        [environments]
        build = ["build"]
    "#,
                &["build"],
                None,
                None
            ),
            vec![r#"echo build"#]
        );
    }

    #[test]
    fn test_multi_env_default() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]

        [tasks]
        start = "hello world"

        [feature.build.tasks]
        build = "echo build"

        [environments]
        build = ["build"]
    "#,
                &["start"],
                None,
                None
            ),
            vec![r#"hello world"#]
        );
    }

    #[test]
    fn test_multi_env_cuda() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64"]

        [tasks]
        train = "python train.py"
        test = "python test.py"
        start = {depends_on = ["train", "test"]}

        [feature.cuda.tasks]
        train = "python train.py --cuda"
        test = "python test.py --cuda"

        [environments]
        cuda = ["cuda"]

    "#,
                &["start"],
                None,
                Some(EnvironmentName::Named("cuda".to_string()))
            ),
            vec![r#"python train.py --cuda"#, r#"python test.py --cuda"#]
        );
    }
}
