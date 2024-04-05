use crate::project::virtual_packages::{
    verify_current_platform_has_required_virtual_packages, VerifyCurrentPlatformError,
};
use crate::project::Environment;
use crate::task::error::AmbiguousTaskError;
use crate::task::task_environment::{FindTaskError, FindTaskSource, SearchEnvironments};
use crate::task::{TaskDisambiguation, TaskName};
use crate::{
    task::{error::MissingTaskError, CmdArgs, Custom, Task},
    Project,
};
use itertools::Itertools;
use miette::Diagnostic;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    env, fmt,
    ops::Index,
};
use thiserror::Error;

/// A task ID is a unique identifier for a [`TaskNode`] in a [`TaskGraph`].
///
/// To get a task from a [`TaskGraph`], you can use the [`TaskId`] as an index.
#[derive(Debug, Clone, Copy, Eq, PartialOrd, PartialEq, Ord, Hash)]
pub struct TaskId(usize);

/// A node in the [`TaskGraph`].
#[derive(Debug)]
pub struct TaskNode<'p> {
    /// The name of the task or `None` if the task is a custom task.
    pub name: Option<TaskName>,

    /// The environment to run the task in
    pub run_environment: Environment<'p>,

    /// A reference to a project task, or a owned custom task.
    pub task: Cow<'p, Task>,

    /// Additional arguments to pass to the command
    pub additional_args: Vec<String>,

    /// The id's of the task that this task depends on.
    pub dependencies: Vec<TaskId>,
}
impl fmt::Display for TaskNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "task: {}, environment: {}, command: `{}`, additional arguments: `{}`, depends_on: `{}`",
            self.name.clone().unwrap_or("CUSTOM COMMAND".into()).0,
            self.run_environment.name(),
            self.task.as_single_command().unwrap_or(Cow::Owned("".to_string())),
            self.additional_args.join(", "),
            self.dependencies
                .iter()
                .map(|id| id.0.to_string())
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
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
#[derive(Debug)]
pub struct TaskGraph<'p> {
    /// The project that this graph references
    project: &'p Project,

    /// The tasks in the graph
    nodes: Vec<TaskNode<'p>>,
}
impl fmt::Display for TaskGraph<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "TaskGraph: number of nodes: {}, nodes: {}",
            self.nodes.len(),
            self.nodes.iter().format("\n")
        )
    }
}

impl<'p> Index<TaskId> for TaskGraph<'p> {
    type Output = TaskNode<'p>;

    fn index(&self, index: TaskId) -> &Self::Output {
        &self.nodes[index.0]
    }
}

impl<'p> TaskGraph<'p> {
    pub fn project(&self) -> &'p Project {
        self.project
    }

    /// Constructs a new [`TaskGraph`] from a list of command line arguments.
    pub fn from_cmd_args<D: TaskDisambiguation<'p>>(
        project: &'p Project,
        search_envs: &SearchEnvironments<'p, D>,
        args: Vec<String>,
    ) -> Result<Self, TaskGraphError> {
        let mut args = args;

        if let Some(name) = args.first() {
            match search_envs.find_task(TaskName(name.clone()), FindTaskSource::CmdArgs) {
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
                            name: Some(args.remove(0).into()),
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
        verify_current_platform_has_required_virtual_packages(&run_environment)?;
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
    fn from_root<D: TaskDisambiguation<'p>>(
        project: &'p Project,
        search_environments: &SearchEnvironments<'p, D>,
        root: TaskNode<'p>,
    ) -> Result<Self, TaskGraphError> {
        let mut task_name_to_node: HashMap<TaskName, TaskId> =
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
                let node = &nodes[next_node_to_visit];
                let (task_env, task_dependency) = match search_environments.find_task(
                    dependency.clone(),
                    FindTaskSource::DependsOn(
                        node.name
                            .clone()
                            .expect("only named tasks can have dependencies"),
                        match &node.task {
                            Cow::Borrowed(task) => task,
                            Cow::Owned(_) => {
                                unreachable!("only named tasks can have dependencies")
                            }
                        },
                    ),
                ) {
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
                task_name_to_node.insert(dependency.clone(), task_id);

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

    #[error(transparent)]
    #[diagnostic(transparent)]
    UnsupportedPlatform(#[from] VerifyCurrentPlatformError),
}

#[cfg(test)]
mod test {
    use crate::task::task_environment::SearchEnvironments;
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
        let project = Project::from_str(Path::new("pixi.toml"), project_str).unwrap();

        let environment = environment_name.map(|name| project.environment(&name).unwrap());
        let search_envs = SearchEnvironments::from_opt_env(&project, environment, platform);

        let graph = TaskGraph::from_cmd_args(
            &project,
            &search_envs,
            run_args.iter().map(|arg| arg.to_string()).collect(),
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
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
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
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
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
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
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
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64", "linux-riscv64"]
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
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]

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
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]

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
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]

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

    #[test]
    fn test_multi_env_defaults() {
        // It should select foobar and foo in the default environment
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]

        [tasks]
        foo = "echo foo"
        foobar = { cmd = "echo bar", depends_on = ["foo"] }

        [feature.build.tasks]
        build = "echo build"

        [environments]
        build = ["build"]
    "#,
                &["foobar"],
                None,
                None
            ),
            vec![r#"echo foo"#, r#"echo bar"#]
        );
    }

    #[test]
    #[should_panic]
    fn test_multi_env_defaults_ambigu() {
        // As foo is really ambiguous it should panic
        commands_in_order(
            r#"
        [project]
        name = "pixi"
        channels = ["conda-forge"]
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64", "linux-riscv64"]

        [tasks]
        foo = "echo foo"
        foobar = { cmd = "echo bar", depends_on = ["foo"] }

        [feature.build.tasks]
        build = "echo build"
        foo = "echo foo abmiguity"

        [environments]
        build = ["build"]
    "#,
            &["foobar"],
            None,
            None,
        );
    }
}
