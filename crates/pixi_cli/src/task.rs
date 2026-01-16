use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    io::Write,
    path::PathBuf,
    str::FromStr,
};

use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use pixi_api::WorkspaceContext;
use pixi_manifest::{
    EnvironmentName, FeatureName,
    task::{Alias, CmdArgs, Dependency, Execute, Task, TaskArg, TaskName, quote},
};
use rattler_conda_types::Platform;
use serde::Serialize;
use serde_with::serde_as;

use pixi_core::{Workspace, WorkspaceLocator, workspace::Environment};

use crate::{cli_config::WorkspaceConfig, cli_interface::CliInterface};

// Type alias for the common pattern of tasks organized by environment
pub type TasksPerEnvironment = HashMap<EnvironmentName, HashMap<TaskName, Task>>;

// Group summary: (name, description, count)
pub type GroupSummary = (String, Option<String>, usize);

// Hint messages for group display
const GROUP_HINT_SHORT: &str = "Use --all or --group <NAME>";
const GROUP_HINT_FULL: &str = "Use `pixi task list --all` or `pixi task list --group <NAME>`";

/// Collect tasks from a single environment, returning None if no tasks exist.
fn collect_tasks_from_env(env: &Environment) -> Option<(EnvironmentName, HashMap<TaskName, Task>)> {
    let best_platform = env.best_platform();
    let task_map: HashMap<TaskName, Task> = env
        .get_filtered_tasks()
        .into_iter()
        .filter_map(|name| {
            env.task(&name, Some(best_platform))
                .ok()
                .map(|task| (name, task.clone()))
        })
        .collect();

    if task_map.is_empty() {
        None
    } else {
        Some((env.name().clone(), task_map))
    }
}

/// Collect tasks with their definitions from the workspace.
/// If an explicit environment is provided, only tasks from that environment are collected.
/// Otherwise, tasks from all environments are collected.
pub fn collect_tasks_with_definitions<'p>(
    workspace: &'p Workspace,
    explicit_environment: Option<&Environment<'p>>,
) -> TasksPerEnvironment {
    if let Some(env) = explicit_environment {
        collect_tasks_from_env(env)
            .map(|(name, tasks)| HashMap::from([(name, tasks)]))
            .unwrap_or_default()
    } else {
        workspace
            .environments()
            .iter()
            .filter_map(collect_tasks_from_env)
            .collect()
    }
}

#[derive(Parser, Debug)]
pub enum Operation {
    /// Add a command to the workspace
    #[clap(visible_alias = "a")]
    Add(AddArgs),

    /// Remove a command from the workspace
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),

    /// Alias another specific command
    #[clap(alias = "@")]
    Alias(AliasArgs),

    /// List all tasks in the workspace
    #[clap(visible_alias = "ls", alias = "l")]
    List(ListArgs),
}

#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct RemoveArgs {
    /// Task name to remove.
    #[arg(value_name = "TASK_NAME")]
    pub names: Vec<TaskName>,

    /// The platform for which the task should be removed.
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The feature for which the task should be removed.
    #[arg(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct AddArgs {
    /// Task name.
    pub name: TaskName,

    /// One or more commands to actually execute.
    #[clap(required = true, num_args = 1.., id = "COMMAND")]
    pub commands: Vec<String>,

    /// Depends on these other commands.
    #[clap(long)]
    #[clap(num_args = 1..)]
    pub depends_on: Option<Vec<Dependency>>,

    /// The platform for which the task should be added.
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The feature for which the task should be added.
    #[arg(long, short)]
    pub feature: Option<String>,

    /// The working directory relative to the root of the workspace.
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// The environment variable to set, use --env key=value multiple times for
    /// more than one variable.
    #[arg(long, value_parser = parse_key_val)]
    pub env: Vec<(String, String)>,

    /// Add a default environment for the task.
    #[arg(long)]
    pub default_environment: Option<EnvironmentName>,

    /// A description of the task to be added.
    #[arg(long)]
    pub description: Option<String>,

    /// Isolate the task from the shell environment, and only use the pixi
    /// environment to run the task.
    #[arg(long)]
    pub clean_env: bool,

    /// The arguments to pass to the task
    #[arg(long = "arg", action = clap::ArgAction::Append)]
    pub args: Option<Vec<TaskArg>>,
}

/// Parse a single key-value pair
fn parse_key_val(s: &str) -> Result<(String, String), Box<dyn Error + Send + Sync + 'static>> {
    let pos = s
        .find('=')
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{s}`"))?;
    let key = s[..pos].to_string();
    let value = s[pos + 1..].to_string();
    Ok((key, value))
}

#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct AliasArgs {
    /// Alias name
    pub alias: TaskName,

    /// Depends on these tasks to execute
    #[clap(required = true, num_args = 1..)]
    pub depends_on: Vec<Dependency>,

    /// The platform for which the alias should be added
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The description of the alias task
    #[arg(long)]
    pub description: Option<String>,
}

#[derive(Parser, Debug, Clone)]
pub struct ListArgs {
    /// Tasks available for this machine per environment
    #[arg(long, short)]
    pub summary: bool,

    /// Output the list of tasks from all environments in
    /// machine readable format (space delimited)
    /// this output is used for autocomplete by `pixi run`
    #[arg(long, hide(true))]
    pub machine_readable: bool,

    /// The environment the list should be generated for.
    /// If not specified, the default environment is used.
    #[arg(long, short)]
    pub environment: Option<String>,

    /// List as json instead of a tree
    /// If not specified, the default environment is used.
    #[arg(long)]
    pub json: bool,

    /// Show only tasks belonging to the specified group
    #[arg(long, short = 'g')]
    pub group: Option<String>,

    /// Show all tasks including those in groups
    #[arg(long)]
    pub all: bool,
}

impl From<AddArgs> for Task {
    fn from(value: AddArgs) -> Self {
        let depends_on = value.depends_on.unwrap_or_default();
        // description or none
        let description = value.description;
        // Convert the arguments into a single string representation
        let cmd_args = value
            .commands
            .iter()
            .exactly_one()
            .map(|c| c.to_string())
            .unwrap_or_else(|_| {
                // Simply concatenate all arguments
                value
                    .commands
                    .iter()
                    .map(|arg| quote(arg).into_owned())
                    .join(" ")
            });

        // Depending on whether the task has a command, and depends_on or not we create
        // a plain or complex, or alias command.
        if cmd_args.trim().is_empty() && !depends_on.is_empty() {
            Self::Alias(Alias {
                depends_on,
                description,
                args: value.args,
                group: None,
                group_description: None,
            })
        } else if depends_on.is_empty()
            && value.cwd.is_none()
            && value.env.is_empty()
            && value.default_environment.is_none()
            && description.is_none()
            && value.args.is_none()
        {
            Self::Plain(cmd_args.into())
        } else {
            let clean_env = value.clean_env;
            let cwd = value.cwd;
            let default_environment = value.default_environment;
            let env = if value.env.is_empty() {
                None
            } else {
                let mut env = IndexMap::new();
                for (key, value) in value.env {
                    env.insert(key, value);
                }
                Some(env)
            };
            let args = value.args;

            Self::Execute(Box::new(Execute {
                cmd: CmdArgs::Single(cmd_args.into()),
                depends_on,
                inputs: None,
                outputs: None,
                cwd,
                env,
                default_environment,
                description,
                clean_env,
                args,
                group: None,
                group_description: None,
            }))
        }
    }
}

impl From<AliasArgs> for Task {
    fn from(value: AliasArgs) -> Self {
        Self::Alias(Alias {
            depends_on: value.depends_on,
            description: value.description,
            args: None,
            group: None,
            group_description: None,
        })
    }
}

/// Interact with tasks in the workspace
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// Add, remove, or update a task
    #[clap(subcommand)]
    pub operation: Operation,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,
}

fn print_heading(value: &str) {
    let bold = console::Style::new().bold();
    eprintln!("{}\n{:-<2$}", bold.apply_to(value), "", value.len(),);
}

/// Create a human-readable representation of a list of tasks.
/// Uses a unified table format with all tasks and their descriptions.
fn print_tasks(task_map: TasksPerEnvironment, summary: bool) -> Result<(), std::io::Error> {
    if summary {
        print_heading("Tasks per environment:");
        for (env, tasks) in task_map {
            let formatted: String = tasks
                .keys()
                .sorted()
                .map(|name| name.fancy_display())
                .join(", ");
            eprintln!("{}: {}", env.fancy_display().bold(), formatted);
        }
        return Ok(());
    }

    let items = collect_unique_tasks_for_display(&task_map);
    if items.is_empty() {
        return Ok(());
    }

    print_heading("Tasks that can run on this machine:");
    print_styled_table("Task", "Description", &items)
}

/// Check if any tasks in the collection have a group assigned
fn any_task_has_group(tasks_per_env: &TasksPerEnvironment) -> bool {
    tasks_per_env
        .values()
        .flat_map(|tasks| tasks.values())
        .any(|task| task.group().is_some())
}

/// Collect unique tasks across environments and format them for table display.
fn collect_unique_tasks_for_display(
    tasks_per_env: &TasksPerEnvironment,
) -> Vec<(String, Option<String>)> {
    let mut all_tasks: BTreeMap<TaskName, Option<String>> = BTreeMap::new();

    for tasks in tasks_per_env.values() {
        for (name, task) in tasks {
            all_tasks
                .entry(name.clone())
                .or_insert_with(|| task.description().map(|s| s.to_string()));
        }
    }

    all_tasks
        .iter()
        .map(|(name, desc)| (name.fancy_display().to_string(), desc.clone()))
        .collect()
}

/// Print a styled table with name/description pairs
fn print_styled_table(
    header_left: &str,
    header_right: &str,
    items: &[(String, Option<String>)],
) -> std::io::Result<()> {
    let mut writer = tabwriter::TabWriter::new(std::io::stdout());
    let header_style = console::Style::new().bold().cyan();

    writeln!(
        writer,
        "{}\t{}",
        header_style.apply_to(header_left),
        header_style.apply_to(header_right)
    )?;

    for (name, description) in items {
        let desc_display = match description {
            Some(desc) => console::style(desc).italic().to_string(),
            None => String::new(),
        };
        writeln!(writer, "{}\t{}", name, desc_display)?;
    }

    writer.flush().inspect_err(|e| {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
    })?;

    Ok(())
}

/// Print hints about available groups
fn print_group_hints(groups: &[GroupSummary], use_full_command: bool) {
    if groups.is_empty() {
        return;
    }

    let total_grouped: usize = groups.iter().map(|(_, _, count)| count).sum();
    let hint = if use_full_command {
        GROUP_HINT_FULL
    } else {
        GROUP_HINT_SHORT
    };

    eprintln!(
        "\n{} task(s) in {} group(s) not shown. {}.",
        total_grouped,
        groups.len(),
        hint
    );

    // Convert groups to (name, description) pairs for table display
    let items: Vec<(String, Option<String>)> = groups
        .iter()
        .map(|(name, desc, _)| (name.clone(), desc.clone()))
        .collect();

    let _ = print_styled_table("Group", "Description", &items);
}

/// Print message when no ungrouped tasks exist
fn print_no_ungrouped_tasks_hint(use_full_command: bool) {
    let hint = if use_full_command {
        GROUP_HINT_FULL
    } else {
        GROUP_HINT_SHORT
    };
    eprintln!("No ungrouped tasks. {} to see grouped tasks.", hint);
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let workspace_ctx = WorkspaceContext::new(CliInterface {}, workspace.clone());

    match args.operation {
        Operation::Add(args) => add_task(workspace_ctx, args).await,
        Operation::Remove(args) => remove_tasks(workspace_ctx, args).await,
        Operation::Alias(args) => alias_task(workspace_ctx, args).await,
        Operation::List(args) => list_tasks(workspace_ctx, args).await,
    }
}

async fn list_tasks(
    workspace_ctx: WorkspaceContext<CliInterface>,
    args: ListArgs,
) -> miette::Result<()> {
    if args.json {
        return print_tasks_json(workspace_ctx.workspace());
    }

    let tasks_per_env = workspace_ctx
        .list_tasks(
            args.environment
                .and_then(|e| EnvironmentName::from_str(&e.to_string()).ok()),
        )
        .await?;

    if tasks_per_env.is_empty() {
        eprintln!("No tasks found",);
        return Ok(());
    }

    if args.machine_readable {
        let unformatted: String = tasks_per_env
            .iter()
            .flat_map(|(_, v)| v.keys())
            .sorted()
            .map(|name| name.as_str())
            .join(" ");
        writeln!(std::io::stdout(), "{unformatted}")
            .inspect_err(|e| {
                if e.kind() == std::io::ErrorKind::BrokenPipe {
                    std::process::exit(0);
                }
            })
            .into_diagnostic()?;

        return Ok(());
    }

    let has_groups = any_task_has_group(&tasks_per_env);

    // Handle group filtering
    if let Some(group_filter) = &args.group {
        // Filter to only tasks in the specified group
        let filtered: TasksPerEnvironment = tasks_per_env
            .into_iter()
            .map(|(env, tasks)| {
                let filtered_tasks: HashMap<TaskName, Task> = tasks
                    .into_iter()
                    .filter(|(_, task)| task.group() == Some(group_filter.as_str()))
                    .collect();
                (env, filtered_tasks)
            })
            .filter(|(_, tasks)| !tasks.is_empty())
            .collect();

        if filtered.is_empty() {
            eprintln!("No tasks found in group '{}'", group_filter);
            return Ok(());
        }

        print_tasks(filtered, args.summary).into_diagnostic()?;
    } else if args.all && has_groups {
        // Show all tasks organized by group (only if groups exist)
        print_tasks_by_group(tasks_per_env, args.summary).into_diagnostic()?;
    } else if has_groups {
        // Default with groups: show ungrouped tasks, then hint about groups
        let (ungrouped, groups) = partition_tasks_by_group(&tasks_per_env);
        let ungrouped_empty = ungrouped.is_empty();

        if !ungrouped_empty {
            print_tasks(ungrouped, args.summary).into_diagnostic()?;
        }

        print_group_hints(&groups, false);

        if ungrouped_empty {
            print_no_ungrouped_tasks_hint(false);
        }
    } else {
        // No groups exist - just show all tasks normally
        print_tasks(tasks_per_env, args.summary).into_diagnostic()?;
    }

    Ok(())
}

/// Partition tasks into ungrouped and grouped, returning group info (name, description, count)
fn partition_tasks_by_group(
    tasks_per_env: &TasksPerEnvironment,
) -> (TasksPerEnvironment, Vec<GroupSummary>) {
    let mut ungrouped: TasksPerEnvironment = HashMap::new();
    // Track count and description for each group
    let mut group_info: HashMap<String, (usize, Option<String>)> = HashMap::new();

    for (env, tasks) in tasks_per_env {
        let mut env_ungrouped: HashMap<TaskName, Task> = HashMap::new();

        for (name, task) in tasks {
            if let Some(group) = task.group() {
                let entry = group_info.entry(group.to_string()).or_insert((0, None));
                entry.0 += 1;
                // Grab description from first task that has one
                if entry.1.is_none() {
                    if let Some(desc) = task.group_description() {
                        entry.1 = Some(desc.to_string());
                    }
                }
            } else {
                env_ungrouped.insert(name.clone(), task.clone());
            }
        }

        if !env_ungrouped.is_empty() {
            ungrouped.insert(env.clone(), env_ungrouped);
        }
    }

    // Convert to vec with (name, description, count)
    let groups: Vec<GroupSummary> = group_info
        .into_iter()
        .map(|(name, (count, desc))| (name, desc, count))
        .collect();

    (ungrouped, groups)
}

/// Print tasks organized by group
fn print_tasks_by_group(tasks_per_env: TasksPerEnvironment, summary: bool) -> std::io::Result<()> {
    // Collect all tasks across environments, grouped
    let mut ungrouped: HashMap<TaskName, Task> = HashMap::new();
    let mut grouped: HashMap<String, HashMap<TaskName, Task>> = HashMap::new();

    for (_env, tasks) in tasks_per_env {
        for (name, task) in tasks {
            if let Some(group) = task.group() {
                grouped
                    .entry(group.to_string())
                    .or_default()
                    .insert(name, task);
            } else {
                ungrouped.insert(name, task);
            }
        }
    }

    let mut writer = tabwriter::TabWriter::new(std::io::stdout());

    // Print ungrouped tasks first
    if !ungrouped.is_empty() {
        writeln!(writer, "Tasks:")?;
        for (name, task) in &ungrouped {
            let description = task.description().unwrap_or("");
            if summary {
                writeln!(writer, "  {}", name.fancy_display())?;
            } else {
                writeln!(writer, "  {}\t{}", name.fancy_display(), description)?;
            }
        }
    }

    // Print grouped tasks
    for (group_name, tasks) in &grouped {
        writeln!(writer)?;
        writeln!(writer, "{}:", group_name)?;
        for (name, task) in tasks {
            let description = task.description().unwrap_or("");
            if summary {
                writeln!(writer, "  {}", name.fancy_display())?;
            } else {
                writeln!(writer, "  {}\t{}", name.fancy_display(), description)?;
            }
        }
    }

    writer.flush()?;
    Ok(())
}

/// Display available tasks with descriptions and hints about grouped tasks.
/// This is used by both `pixi task list` and `pixi run` (when no task is specified).
pub fn display_available_tasks_with_hints(tasks_per_env: &TasksPerEnvironment) {
    if tasks_per_env.is_empty() {
        return;
    }

    if any_task_has_group(tasks_per_env) {
        // Show ungrouped tasks with hints about groups
        let (ungrouped, groups) = partition_tasks_by_group(tasks_per_env);

        if !ungrouped.is_empty() {
            display_task_list_with_descriptions(&ungrouped);
        }

        print_group_hints(&groups, true);

        if ungrouped.is_empty() && !groups.is_empty() {
            print_no_ungrouped_tasks_hint(true);
        }
    } else {
        // No groups - show all tasks with descriptions
        display_task_list_with_descriptions(tasks_per_env);
    }
}

/// Display a simple list of tasks with their descriptions (used for `pixi run` hint output)
fn display_task_list_with_descriptions(tasks_per_env: &TasksPerEnvironment) {
    let items = collect_unique_tasks_for_display(tasks_per_env);
    if items.is_empty() {
        return;
    }

    eprintln!("\nAvailable tasks:");
    let _ = print_styled_table("Task", "Description", &items);
}

async fn add_task(
    workspace_ctx: WorkspaceContext<CliInterface>,
    args: AddArgs,
) -> miette::Result<()> {
    let feature = args
        .clone()
        .feature
        .map_or_else(FeatureName::default, FeatureName::from);

    workspace_ctx
        .add_task(
            args.name.clone(),
            args.clone().into(),
            feature,
            args.platform,
        )
        .await?;

    Ok(())
}

async fn alias_task(
    workspace_ctx: WorkspaceContext<CliInterface>,
    args: AliasArgs,
) -> miette::Result<()> {
    workspace_ctx
        .alias_task(args.clone().alias, args.clone().into(), args.platform)
        .await?;

    Ok(())
}

async fn remove_tasks(
    workspace_ctx: WorkspaceContext<CliInterface>,
    args: RemoveArgs,
) -> miette::Result<()> {
    workspace_ctx
        .remove_task(
            args.names,
            args.platform,
            args.feature
                .map_or_else(FeatureName::default, FeatureName::from),
        )
        .await
}

fn print_tasks_json(project: &Workspace) -> miette::Result<()> {
    let env_feature_task_map: Vec<EnvTasks> = build_env_feature_task_map(project);

    let json_string =
        serde_json::to_string_pretty(&env_feature_task_map).expect("Failed to serialize tasks");
    writeln!(std::io::stdout(), "{json_string}")
        .inspect_err(|e| {
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                std::process::exit(0);
            }
        })
        .into_diagnostic()?;

    Ok(())
}

fn build_env_feature_task_map(project: &Workspace) -> Vec<EnvTasks> {
    project
        .environments()
        .iter()
        .sorted_by_key(|env| env.name().to_string())
        .map(EnvTasks::from)
        .collect()
}

#[derive(Serialize, Debug)]
struct EnvTasks {
    environment: String,
    features: Vec<SerializableFeature>,
}

impl From<&Environment<'_>> for EnvTasks {
    fn from(env: &Environment) -> Self {
        Self {
            environment: env.name().to_string(),
            features: env
                .feature_tasks()
                .iter()
                .map(|(feature_name, task_map)| {
                    SerializableFeature::from((*feature_name, task_map))
                })
                .collect(),
        }
    }
}

#[derive(Serialize, Debug)]
struct SerializableFeature {
    name: String,
    tasks: Vec<SerializableTask>,
}

#[derive(Serialize, Debug)]
struct SerializableTask {
    name: String,
    #[serde(flatten)]
    info: TaskInfo,
}

impl From<(&FeatureName, &HashMap<&TaskName, &Task>)> for SerializableFeature {
    fn from((feature_name, task_map): (&FeatureName, &HashMap<&TaskName, &Task>)) -> Self {
        Self {
            name: feature_name.to_string(),
            tasks: task_map
                .iter()
                .map(|(task_name, task)| SerializableTask {
                    name: task_name.to_string(),
                    info: TaskInfo::from(*task),
                })
                .collect(),
        }
    }
}

/// Collection of task properties for displaying in the UI.
#[serde_as]
#[derive(Serialize, Debug)]
pub struct TaskInfo {
    cmd: Option<String>,
    description: Option<String>,
    depends_on: Vec<Dependency>,
    args: Option<Vec<TaskArg>>,
    cwd: Option<PathBuf>,
    env: Option<IndexMap<String, String>>,
    default_environment: Option<EnvironmentName>,
    clean_env: bool,
    inputs: Option<Vec<String>>,
    outputs: Option<Vec<String>>,
}

impl From<&Task> for TaskInfo {
    fn from(task: &Task) -> Self {
        TaskInfo {
            cmd: task
                .as_single_command_no_render()
                .ok()
                .and_then(|cmd| cmd.map(|c| c.to_string())),
            description: task.description().map(|desc| desc.to_string()),
            depends_on: task.depends_on().to_vec(),
            args: task.args().map(|args| args.to_vec()),
            cwd: task.working_directory().map(PathBuf::from),
            env: task.env().cloned(),
            default_environment: task.default_environment().cloned(),
            clean_env: task.clean_env(),
            inputs: task.inputs().map(|inputs| {
                inputs
                    .iter()
                    .map(|input| input.source().to_string())
                    .collect()
            }),
            outputs: task.outputs().map(|outputs| {
                outputs
                    .iter()
                    .map(|output| output.source().to_string())
                    .collect()
            }),
        }
    }
}
