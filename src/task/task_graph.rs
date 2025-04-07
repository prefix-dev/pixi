use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    env,
    fmt::{self, Display},
    ops::Index,
};

use itertools::Itertools;
use miette::Diagnostic;
use pixi_manifest::{
    task::{CmdArgs, Custom, Dependency},
    EnvironmentName, Task, TaskName,
};
use thiserror::Error;

use crate::{
    task::{
        error::{AmbiguousTaskError, MissingTaskError},
        task_environment::{FindTaskError, FindTaskSource, SearchEnvironments},
        TaskDisambiguation,
    },
    workspace::Environment,
    Workspace,
};

/// A task ID is a unique identifier for a [`TaskNode`] in a [`TaskGraph`].
///
/// To get a task from a [`TaskGraph`], you can use the [`TaskId`] as an index.
#[derive(Debug, Clone, Copy, Eq, PartialOrd, PartialEq, Ord, Hash)]
pub struct TaskId(usize);

/// A dependency is a task name and a list of arguments.
#[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub struct GraphDependency(TaskId, Option<Vec<String>>, Option<EnvironmentName>);

impl GraphDependency {
    pub fn task_id(&self) -> TaskId {
        self.0
    }
}

/// A node in the [`TaskGraph`].
#[derive(Debug)]
pub struct TaskNode<'p> {
    /// The name of the task or `None` if the task is a custom task.
    pub name: Option<TaskName>,

    /// The environment to run the task in
    pub run_environment: Environment<'p>,

    /// A reference to a project task, or a owned custom task.
    pub task: Cow<'p, Task>,

    /// Additional arguments to pass to the command. These arguments are passed
    /// verbatim, e.g. they will not be interpreted by deno.
    pub additional_args: Option<Vec<String>>,

    /// The arguments to pass to the dependencies.
    pub arguments_values: Option<Vec<String>>,

    /// The id's of the task that this task depends on.
    pub dependencies: Vec<GraphDependency>,
}

impl fmt::Display for TaskNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "task: {}, environment: {}, command: `{}`, additional arguments: `{}`, depends-on: `{}`",
            self.name.clone().unwrap_or("CUSTOM COMMAND".into()),
            self.run_environment.name(),
            self.task.as_single_command().unwrap_or(Cow::Owned("".to_string())),
            self.format_additional_args(),
            self.dependencies
                .iter()
                .map(|id| format!("{:?}", id.task_id()))
                .collect::<Vec<String>>()
                .join(", ")
        )
    }
}

impl TaskNode<'_> {
    /// Returns the full command that should be executed for this task. This
    /// includes any additional arguments that should be passed to the
    /// command.
    ///
    /// This function returns `None` if the task does not define a command to
    /// execute. This is the case for alias only commands.
    #[cfg(test)]
    pub(crate) fn full_command(&self) -> Option<String> {
        let mut cmd = self.task.as_single_command()?.to_string();

        if let Some(additional_args) = &self.additional_args {
            if !additional_args.is_empty() {
                // Pass each additional argument varbatim by wrapping it in single quotes
                cmd.push_str(&format!(" {}", self.format_additional_args()));
            }
        }

        Some(cmd)
    }

    /// Format the additional arguments passed to this command
    fn format_additional_args(&self) -> Box<dyn Display + '_> {
        if let Some(additional_args) = &self.additional_args {
            Box::new(
                additional_args
                    .iter()
                    .format_with(" ", |arg, f| f(&format_args!("'{}'", arg))),
            )
        } else {
            Box::new("".to_string())
        }
    }
}

/// A [`TaskGraph`] is a graph of tasks that defines the relationships between
/// different executable tasks.
#[derive(Debug)]
pub struct TaskGraph<'p> {
    /// The project that this graph references
    project: &'p Workspace,

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
    pub(crate) fn project(&self) -> &'p Workspace {
        self.project
    }

    /// Constructs a new [`TaskGraph`] from a list of command line arguments.
    pub fn from_cmd_args<D: TaskDisambiguation<'p>>(
        project: &'p Workspace,
        search_envs: &SearchEnvironments<'p, D>,
        args: Vec<String>,
        skip_deps: bool,
    ) -> Result<Self, TaskGraphError> {
        // Split 'args' into arguments if it's a single string, supporting commands
        // like: `"test 1 == 0 || echo failed"` or `"echo foo && echo bar"` or
        // `"echo 'Hello World'"` This prevents shell interpretation of pixi run
        // inputs. Use as-is if 'task' already contains multiple elements.
        let (mut args, verbatim) = if args.len() == 1 {
            (
                shlex::split(args[0].as_str()).ok_or(TaskGraphError::InvalidTask)?,
                false,
            )
        } else {
            (args, true)
        };

        if let Some(name) = args.first() {
            match search_envs.find_task(TaskName::from(name.clone()), FindTaskSource::CmdArgs, None)
            {
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

                    let task_name = args.remove(0);

                    let (additional_args, arguments_values) = if let Some(argument_map) =
                        task.get_args()
                    {
                        // Check if we don't have more arguments than the task expects
                        if args.len() > argument_map.len() {
                            return Err(TaskGraphError::TooManyArguments(task_name.to_string()));
                        }

                        (None, Some(args))
                    } else {
                        (Some(args), None)
                    };

                    if skip_deps {
                        return Ok(Self {
                            project,
                            nodes: vec![TaskNode {
                                name: Some(task_name.into()),
                                task: Cow::Borrowed(task),
                                run_environment: run_env,
                                additional_args,
                                arguments_values,
                                dependencies: vec![],
                            }],
                        });
                    }

                    return Self::from_root(
                        project,
                        search_envs,
                        TaskNode {
                            name: Some(task_name.into()),
                            task: Cow::Borrowed(task),
                            run_environment: run_env,
                            additional_args,
                            arguments_values,
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

        // Depending on whether we are passing arguments verbatim or now we allow deno
        // to interpret them or not.
        let (cmd, additional_args) = if verbatim {
            let mut args = args.into_iter();
            (
                CmdArgs::Single(args.next().expect("must be at least one argument")),
                args.collect(),
            )
        } else {
            (CmdArgs::Multiple(args), vec![])
        };

        Self::from_root(
            project,
            search_envs,
            TaskNode {
                name: None,
                task: Cow::Owned(
                    Custom {
                        cmd,
                        cwd: env::current_dir().ok(),
                    }
                    .into(),
                ),
                run_environment,
                additional_args: Some(additional_args),
                arguments_values: None,
                dependencies: vec![],
            },
        )
    }

    /// Constructs a new instance of a [`TaskGraph`] from a root task.
    fn from_root<D: TaskDisambiguation<'p>>(
        project: &'p Workspace,
        search_environments: &SearchEnvironments<'p, D>,
        root: TaskNode<'p>,
    ) -> Result<Self, TaskGraphError> {
        let mut task_name_with_args_to_node: HashMap<Dependency, TaskId> =
            HashMap::from_iter(root.name.clone().into_iter().map(|name| {
                (
                    Dependency::new_without_env(&name.to_string(), root.arguments_values.clone()),
                    TaskId(0),
                )
            }));
        let mut nodes = vec![root];

        // Iterate over all the nodes in the graph and add them to the graph.
        let mut next_node_to_visit = 0;
        while next_node_to_visit < nodes.len() {
            let dependencies =
                Vec::from_iter(nodes[next_node_to_visit].task.depends_on().iter().cloned());

            // Collect all dependency data before modifying nodes
            let mut deps_to_process = Vec::new();

            // Iterate over all the dependencies of the node and add them to the graph.
            let mut node_dependencies = Vec::with_capacity(dependencies.len());
            for dependency in dependencies {
                // Check if we visited this node before already.
                if let Some(&task_id) = task_name_with_args_to_node.get(&dependency) {
                    node_dependencies.push(GraphDependency(
                        task_id,
                        dependency.args.clone(),
                        dependency.environment.clone(),
                    ));
                    continue;
                }

                // Find the task in the project
                let node = &nodes[next_node_to_visit];

                // Clone what we need before modifying nodes
                let node_name = node
                    .name
                    .clone()
                    .expect("only named tasks can have dependencies");
                let task_ref = match &node.task {
                    Cow::Borrowed(task) => task,
                    Cow::Owned(_) => unreachable!("only named tasks can have dependencies"),
                };

                let task_specific_environment = dependency
                    .environment
                    .clone()
                    .and_then(|environment| project.environment(&environment));

                let (task_env, task_dependency) = match search_environments.find_task(
                    dependency.task_name.clone(),
                    FindTaskSource::DependsOn(node_name, task_ref),
                    task_specific_environment,
                ) {
                    Err(FindTaskError::MissingTask(err)) => {
                        return Err(TaskGraphError::MissingTask(err))
                    }
                    Err(FindTaskError::AmbiguousTask(err)) => {
                        return Err(TaskGraphError::AmbiguousTask(err))
                    }
                    Ok(result) => result,
                };

                // Store the dependency data for processing later
                deps_to_process.push((dependency, task_env, task_dependency));
            }

            // Process all dependencies after collecting them
            for (dependency, task_env, task_dependency) in deps_to_process {
                // Add the node to the graph
                let task_id = TaskId(nodes.len());
                nodes.push(TaskNode {
                    name: Some(dependency.task_name.clone()),
                    task: Cow::Borrowed(task_dependency),
                    run_environment: task_env,
                    additional_args: Some(Vec::new()),
                    arguments_values: dependency.args.clone(),
                    dependencies: Vec::new(),
                });

                // Store the task id in the map to be able to look up the name later
                task_name_with_args_to_node.insert(dependency.clone(), task_id);

                // Add the dependency to the node
                node_dependencies.push(GraphDependency(
                    task_id,
                    dependency.args.clone(),
                    dependency.environment.clone(),
                ));
            }

            nodes[next_node_to_visit].dependencies = node_dependencies;
            next_node_to_visit += 1;
        }

        Ok(Self { project, nodes })
    }

    /// Returns the topological order of the tasks in the graph.
    ///
    /// The topological order is the order in which the tasks should be executed
    /// to ensure that all dependencies of a task are executed before the
    /// task itself.
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
                visit(dependency.task_id(), nodes, visited, order);
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

    #[error("could not split task, assuming non valid task")]
    InvalidTask,

    #[error("task '{0}' received more arguments than expected")]
    TooManyArguments(String),
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use pixi_manifest::EnvironmentName;
    use rattler_conda_types::Platform;

    use crate::{
        task::{task_environment::SearchEnvironments, task_graph::TaskGraph},
        Workspace,
    };

    fn commands_in_order(
        project_str: &str,
        run_args: &[&str],
        platform: Option<Platform>,
        environment_name: Option<EnvironmentName>,
        skip_deps: bool,
    ) -> Vec<String> {
        let project = Workspace::from_str(Path::new("pixi.toml"), project_str).unwrap();

        let environment = environment_name.map(|name| project.environment(&name).unwrap());
        let search_envs = SearchEnvironments::from_opt_env(&project, environment, platform);

        let graph = TaskGraph::from_cmd_args(
            &project,
            &search_envs,
            run_args.iter().map(|arg| arg.to_string()).collect(),
            skip_deps,
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
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
        [tasks]
        root = "echo root"
        task1 = {cmd="echo task1", depends-on=["root"]}
        task2 = {cmd="echo task2", depends-on=["root"]}
        top = {cmd="echo top", depends-on=["task1","task2"]}
    "#,
                &["top", "--test"],
                None,
                None,
                false
            ),
            vec!["echo root", "echo task1", "echo task2", "echo top '--test'"]
        );
    }

    #[test]
    fn test_cycle_ordered_commands() {
        assert_eq!(
            commands_in_order(
                r#"
        [project]
        name = "pixi"
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
        [tasks]
        root = {cmd="echo root", depends-on=["task1"]}
        task1 = {cmd="echo task1", depends-on=["root"]}
        task2 = {cmd="echo task2", depends-on=["root"]}
        top = {cmd="echo top", depends-on=["task1","task2"]}
    "#,
                &["top"],
                None,
                None,
                false
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
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]
        [tasks]
        root = "echo root"
        task1 = {cmd="echo task1", depends-on=["root"]}
        task2 = {cmd="echo task2", depends-on=["root"]}
        top = {cmd="echo top", depends-on=["task1","task2"]}
        [target.linux-64.tasks]
        root = {cmd="echo linux", depends-on=["task1"]}
    "#,
                &["top"],
                Some(Platform::Linux64),
                None,
                false
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
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64", "linux-riscv64"]
    "#,
                &["echo bla"],
                None,
                None,
                false
            ),
            vec![r#"echo bla"#]
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
                None,
                false
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
        channels = []
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
                None,
                false
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
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]

        [tasks]
        train = "python train.py"
        test = "python test.py"
        start = {depends-on = ["train", "test"]}

        [feature.cuda.tasks]
        train = "python train.py --cuda"
        test = "python test.py --cuda"

        [environments]
        cuda = ["cuda"]

    "#,
                &["start"],
                None,
                Some(EnvironmentName::Named("cuda".to_string())),
                false
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
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64"]

        [tasks]
        foo = "echo foo"
        foobar = { cmd = "echo bar", depends-on = ["foo"] }

        [feature.build.tasks]
        build = "echo build"

        [environments]
        build = ["build"]
    "#,
                &["foobar"],
                None,
                None,
                false
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
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64", "linux-riscv64"]

        [tasks]
        foo = "echo foo"
        foobar = { cmd = "echo bar", depends-on = ["foo"] }

        [feature.build.tasks]
        build = "echo build"
        foo = "echo foo abmiguity"

        [environments]
        build = ["build"]
    "#,
            &["foobar"],
            None,
            None,
            false,
        );
    }

    #[test]
    fn test_skip_deps() {
        let project = r#"
        [project]
        name = "pixi"
        channels = []
        platforms = ["linux-64", "osx-64", "win-64", "osx-arm64", "linux-riscv64"]

        [tasks]
        foo = "echo foo"
        bar = { cmd = "echo bar", depends-on = ["foo"] }
    "#;
        assert_eq!(
            commands_in_order(project, &["bar"], None, None, true),
            vec![r#"echo bar"#]
        );
        assert_eq!(
            commands_in_order(project, &["bar"], None, None, false),
            vec!["echo foo", "echo bar"]
        );
    }
}
