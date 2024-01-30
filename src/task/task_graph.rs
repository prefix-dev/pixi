use crate::{
    task::{error::MissingTaskError, CmdArgs, Custom, Task},
    Project,
};
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

impl<'p> TaskGraph<'p> {
    pub fn project(&self) -> &'p Project {
        self.project
    }

    /// Constructs a new [`TaskGraph`] from a list of command line arguments.
    pub fn from_cmd_args(
        project: &'p Project,
        args: Vec<String>,
        platform: Option<Platform>,
    ) -> Result<Self, TaskGraphError> {
        let mut args = args;

        if let Some(name) = args.first() {
            // Find the task in the project. First searches for platform specific tasks and falls
            // back to looking for the task in the default tasks.
            if let Some(task) = project.task_opt(name, platform) {
                return Self::from_root(
                    project,
                    platform,
                    TaskNode {
                        name: Some(args.remove(0)),
                        task: Cow::Borrowed(task),
                        additional_args: args,
                        dependencies: vec![],
                    },
                );
            }
        }

        // When no task is found, just execute the command verbatim.
        Self::from_root(
            project,
            platform,
            TaskNode {
                name: None,
                task: Cow::Owned(
                    Custom {
                        cmd: CmdArgs::from(args),
                        cwd: env::current_dir().ok(),
                    }
                    .into(),
                ),
                additional_args: vec![],
                dependencies: vec![],
            },
        )
    }

    /// Constructs a new instance of a [`TaskGraph`] from a root task.
    fn from_root(
        project: &'p Project,
        platform: Option<Platform>,
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
                // Check if we visisted this node before already.
                if let Some(&task_id) = task_name_to_node.get(&dependency) {
                    node_dependencies.push(task_id);
                    continue;
                }

                // Find the task in the project
                let Some(task_dependency) = project.task_opt(&dependency, platform) else {
                    return Err(TaskGraphError::MissingTask(MissingTaskError {
                        task_name: dependency,
                    }));
                };

                // Add the node to the graph
                let task_id = TaskId(nodes.len());
                nodes.push(TaskNode {
                    name: Some(dependency.clone()),
                    task: Cow::Borrowed(task_dependency),
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
}

#[cfg(test)]
mod test {
    use crate::task::task_graph::TaskGraph;
    use crate::Project;
    use rattler_conda_types::Platform;
    use std::path::Path;

    fn commands_in_order(
        project_str: &str,
        run_args: &[&str],
        platform: Option<Platform>,
    ) -> Vec<String> {
        let project = Project::from_str(Path::new(""), project_str).unwrap();

        let graph = TaskGraph::from_cmd_args(
            &project,
            run_args.into_iter().map(|arg| arg.to_string()).collect(),
            platform,
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
            ),
            vec![r#""echo bla""#]
        );
    }
}
