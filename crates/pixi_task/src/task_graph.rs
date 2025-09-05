use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    env,
    fmt::{self, Display},
    ops::Index,
};

use itertools::Itertools;
use miette::Diagnostic;
use pixi_core::{Workspace, workspace::Environment};
use pixi_manifest::{
    EnvironmentName, Task, TaskName,
    task::{
        ArgValues, CmdArgs, Custom, TaskArg, TemplateStringError, TypedArg, TypedDependency,
        TypedDependencyArg,
    },
};
use thiserror::Error;

use crate::{
    TaskDisambiguation,
    error::{AmbiguousTaskError, MissingTaskError},
    task_environment::{FindTaskError, FindTaskSource, SearchEnvironments},
};

/// A task ID is a unique identifier for a [`TaskNode`] in a [`TaskGraph`].
///
/// To get a task from a [`TaskGraph`], you can use the [`TaskId`] as an index.
#[derive(Debug, Clone, Copy, Eq, PartialOrd, PartialEq, Ord, Hash)]
pub struct TaskId(usize);

/// A dependency is a task name and a list of arguments along with the environment to run the task in.
#[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub struct GraphDependency(
    TaskId,
    Option<Vec<TypedDependencyArg>>,
    Option<EnvironmentName>,
);

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
    pub args: Option<ArgValues>,

    /// The id's of the task that this task depends on.
    pub dependencies: Vec<GraphDependency>,
}

impl fmt::Display for TaskNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "task: {}",
            self.name.clone().unwrap_or("CUSTOM COMMAND".into())
        )?;
        write!(f, ", environment: {}", self.run_environment.name())?;
        if let Ok(Some(command)) = self.task.as_single_command(self.args.as_ref()) {
            write!(f, "command: `{command}`,",)?;
        }
        write!(
            f,
            ", additional arguments: `{}`",
            self.format_additional_args()
        )?;
        write!(
            f,
            ", depends-on: `{}`",
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
    pub(crate) fn full_command(&self) -> miette::Result<Option<String>> {
        let mut cmd = self.task.as_single_command(self.args.as_ref())?;

        if let Some(ArgValues::FreeFormArgs(additional_args)) = &self.args {
            if !additional_args.is_empty() {
                // Pass each additional argument varbatim by wrapping it in single quotes
                let formatted_args = format!(" {}", self.format_additional_args());
                cmd = match cmd {
                    Some(Cow::Borrowed(s)) => Some(Cow::Owned(format!("{}{}", s, formatted_args))),
                    Some(Cow::Owned(mut s)) => {
                        s.push_str(&formatted_args);
                        Some(Cow::Owned(s))
                    }
                    None => None,
                };
            }
        }

        Ok(cmd.map(|c| c.into_owned()))
    }

    /// Format the additional arguments passed to this command
    fn format_additional_args(&self) -> Box<dyn Display + '_> {
        if let Some(ArgValues::FreeFormArgs(additional_args)) = &self.args {
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
                    return Err(TaskGraphError::AmbiguousTask(err));
                }
                Ok((task_env, task)) => {
                    // If an explicit environment was specified and the task is from the default
                    // environment use the specified environment instead.
                    let run_env = match search_envs.explicit_environment.clone() {
                        Some(explicit_env) if task_env.is_default() => explicit_env,
                        _ => task_env,
                    };

                    let task_name = args.remove(0);

                    let arg_values = if let Some(task_arguments) = task.args() {
                        // Check if we don't have more arguments than the task expects
                        if args.len() > task_arguments.len() {
                            return Err(TaskGraphError::TooManyArguments(task_name.to_string()));
                        }

                        // TODO: support named arguments from the CLI
                        let typed_dep_args = args
                            .iter()
                            .map(|a| TypedDependencyArg::Positional(a.to_string()))
                            .collect();

                        Some(Self::merge_args(
                            &TaskName::from(task_name.clone()),
                            Some(&task_arguments.to_vec()),
                            Some(&typed_dep_args),
                        )?)
                    } else {
                        Some(ArgValues::FreeFormArgs(args.clone()))
                    };

                    if skip_deps {
                        return Ok(Self {
                            project,
                            nodes: vec![TaskNode {
                                name: Some(task_name.into()),
                                task: Cow::Borrowed(task),
                                run_environment: run_env,
                                args: arg_values,
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
                            args: arg_values,
                            dependencies: vec![],
                        },
                        Some(
                            args.iter()
                                .map(|a| TypedDependencyArg::Positional(a.clone()))
                                .collect(),
                        ),
                    );
                }
            }
        }

        // When no task is found, just execute the command verbatim.
        let run_environment = search_envs
            .explicit_environment
            .clone()
            .unwrap_or_else(|| project.default_environment());

        // For CLI arguments, we want to construct a proper shell command.
        // When we have multiple arguments from CLI, they've already been parsed by the shell
        // and clap, so we reconstruct them into a single shell command to avoid double-quoting.
        let (cmd, additional_args) = if verbatim {
            // Multiple CLI arguments: reconstruct as a single shell command
            let command_string = shlex::try_join(args.iter().map(|s| s.as_str()))
                .map_err(|_| TaskGraphError::InvalidTask)?;
            (CmdArgs::Single(command_string.into()), vec![])
        } else {
            // Single argument that was shell-parsed: use as multiple args
            (
                CmdArgs::Multiple(args.into_iter().map(|arg| arg.into()).collect()),
                vec![],
            )
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
                args: Some(ArgValues::FreeFormArgs(additional_args)),
                dependencies: vec![],
            },
            None,
        )
    }

    /// Constructs a new instance of a [`TaskGraph`] from a root task.
    fn from_root<D: TaskDisambiguation<'p>>(
        project: &'p Workspace,
        search_environments: &SearchEnvironments<'p, D>,
        root: TaskNode<'p>,
        root_args: Option<Vec<TypedDependencyArg>>,
    ) -> Result<Self, TaskGraphError> {
        let mut task_name_with_args_to_node: HashMap<TypedDependency, TaskId> =
            HashMap::from_iter(root.name.clone().into_iter().map(|name| {
                (
                    TypedDependency {
                        task_name: name,
                        args: root_args.clone(),
                        environment: None,
                    },
                    TaskId(0),
                )
            }));
        let mut nodes = vec![root];

        // Iterate over all the nodes in the graph and add them to the graph.
        let mut next_node_to_visit = 0;
        while next_node_to_visit < nodes.len() {
            let node = &nodes[next_node_to_visit];
            let dependencies = Vec::from_iter(node.task.depends_on().iter().cloned());

            // Collect all dependency data before modifying nodes
            let mut deps_to_process: Vec<(TypedDependency, Environment<'p>, &Task)> = Vec::new();

            // Iterate over all the dependencies of the node and add them to the graph.
            let mut node_dependencies = Vec::with_capacity(dependencies.len());
            for dependency in dependencies {
                let dependency = TypedDependency::from_dependency(&dependency, node.args.as_ref())?;
                // Check if we visited this node before already.
                if let Some(&task_id) = task_name_with_args_to_node.get(&dependency) {
                    node_dependencies.push(GraphDependency(
                        task_id,
                        dependency.args.clone(),
                        dependency.environment.clone(),
                    ));
                    continue;
                }

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
                        return Err(TaskGraphError::MissingTask(err));
                    }
                    Err(FindTaskError::AmbiguousTask(err)) => {
                        return Err(TaskGraphError::AmbiguousTask(err));
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
                    args: Some(Self::merge_args(
                        &dependency.task_name,
                        task_dependency.args().map(|args| args.to_vec()).as_ref(),
                        dependency.args.as_ref(),
                    )?),
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

    fn merge_args(
        task_name: &TaskName,
        task_arguments: Option<&Vec<TaskArg>>,
        dep_args: Option<&Vec<TypedDependencyArg>>,
    ) -> Result<ArgValues, TaskGraphError> {
        let task_arguments = match task_arguments {
            Some(args) => args,
            None => &Vec::new(),
        };

        let task_arg_names: Vec<String> = task_arguments
            .iter()
            .map(|arg| arg.name.as_str().to_owned())
            .collect();

        let dep_args = match dep_args {
            Some(args) => args,
            None => &Vec::new(),
        };

        let mut named_args = Vec::new();
        let mut seen_named = false;

        // build up vec of named args whilst validating that all named args are valid for this task,
        // and that all positional args precede any named args
        for arg in dep_args {
            match arg {
                TypedDependencyArg::Named(name, value) => {
                    if !task_arg_names.contains(name) {
                        return Err(TaskGraphError::UnknownArgument(
                            name.to_string(),
                            task_name.to_string(),
                        ));
                    }
                    seen_named = true;
                    named_args.push((name.to_string(), value.to_string()));
                }
                TypedDependencyArg::Positional(value) => {
                    if seen_named {
                        return Err(TaskGraphError::PositionalAfterNamedArgument(
                            value.to_string(),
                            task_name.to_string(),
                        ));
                    }
                }
            }
        }

        let mut typed_args = Vec::with_capacity(task_arguments.len());

        for (i, arg) in task_arguments.iter().enumerate() {
            let arg_name = arg.name.as_str();
            let arg_value = if let Some((_n, v)) = named_args.iter().find(|(n, _v)| n == arg_name) {
                // a matching named arg was specified
                v.to_string()
            } else if i < dep_args.len() {
                // check for a positional arg, or a default value, or error
                match &dep_args[i] {
                    TypedDependencyArg::Positional(v) => v.clone(),
                    _ => {
                        if let Some(default) = &arg.default {
                            default.clone()
                        } else {
                            return Err(TaskGraphError::MissingArgument(
                                arg_name.to_string(),
                                task_name.to_string(),
                            ));
                        }
                    }
                }
            } else if let Some(default) = &arg.default {
                default.clone()
            } else {
                return Err(TaskGraphError::MissingArgument(
                    arg_name.to_owned(),
                    task_name.to_string(),
                ));
            };

            typed_args.push(TypedArg {
                name: arg_name.to_owned(),
                value: arg_value,
            });
        }

        Ok(ArgValues::TypedArgs(typed_args))
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

    #[error("no value provided for argument '{0}' for task '{1}'")]
    MissingArgument(String, String),

    #[error(transparent)]
    #[diagnostic(transparent)]
    TemplateStringError(#[from] TemplateStringError),

    #[error("named argument '{0}' does not exist for task {1}")]
    UnknownArgument(String, String),

    #[error("Positional argument '{0}' found after named argument for task {1}")]
    PositionalAfterNamedArgument(String, String),
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use pixi_core::Workspace;
    use pixi_manifest::EnvironmentName;
    use rattler_conda_types::Platform;

    use crate::{task_environment::SearchEnvironments, task_graph::TaskGraph};

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
            .filter_map(|task| task.full_command().ok().flatten())
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
