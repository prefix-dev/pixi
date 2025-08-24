use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
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
use pixi_manifest::{
    EnvironmentName, FeatureName,
    task::{Alias, CmdArgs, Dependency, Execute, Task, TaskArg, TaskName, quote},
};
use rattler_conda_types::Platform;
use serde::Serialize;
use serde_with::serde_as;

use pixi_core::workspace::virtual_packages::verify_current_platform_can_run_environment;
use pixi_core::{
    Workspace, WorkspaceLocator,
    workspace::{Environment, WorkspaceMut},
};

use crate::cli_config::WorkspaceConfig;

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
        .ok_or_else(|| format!("invalid KEY=value: no `=` found in `{}`", s))?;
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
            })
        } else if depends_on.is_empty()
            && value.cwd.is_none()
            && value.env.is_empty()
            && description.is_none()
            && value.args.is_none()
        {
            Self::Plain(cmd_args.into())
        } else {
            let clean_env = value.clean_env;
            let cwd = value.cwd;
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
                description,
                clean_env,
                args,
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
/// Using a tabwriter for described tasks.
fn print_tasks(
    task_map: HashMap<Environment, HashMap<TaskName, &Task>>,
    summary: bool,
) -> Result<(), std::io::Error> {
    if summary {
        print_heading("Tasks per environment:");
        for (env, tasks) in task_map {
            let formatted: String = tasks
                .keys()
                .sorted()
                .map(|name| name.fancy_display())
                .join(", ");
            eprintln!("{}: {}", env.name().fancy_display().bold(), formatted);
        }
        return Ok(());
    }

    let mut all_tasks: BTreeSet<TaskName> = BTreeSet::new();
    let mut formatted_descriptions: BTreeMap<TaskName, String> = BTreeMap::new();

    task_map.values().for_each(|tasks| {
        tasks.iter().for_each(|(taskname, task)| {
            all_tasks.insert(taskname.clone());
            if let Some(description) = task.description() {
                formatted_descriptions.insert(
                    taskname.clone(),
                    format!("{}", console::style(description).italic()),
                );
            }
        });
    });

    print_heading("Tasks that can run on this machine:");
    let formatted_tasks: String = all_tasks.iter().map(|name| name.fancy_display()).join(", ");
    eprintln!("{}", formatted_tasks);

    let mut writer = tabwriter::TabWriter::new(std::io::stdout());
    let header_style = console::Style::new().bold().cyan();
    let header = format!(
        "{}\t{}",
        header_style.apply_to("Task"),
        header_style.apply_to("Description"),
    );
    writeln!(writer, "{}", &header)?;
    for (taskname, row) in formatted_descriptions {
        writeln!(writer, "{}\t{}", taskname.fancy_display(), row)?;
    }

    writer.flush()
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;
    match args.operation {
        Operation::Add(args) => add_task(workspace.modify()?, args).await,
        Operation::Remove(args) => remove_tasks(workspace.modify()?, args).await,
        Operation::Alias(args) => alias_task(workspace.modify()?, args).await,
        Operation::List(args) => list_tasks(workspace, args).await,
    }
}

async fn list_tasks(workspace: Workspace, args: ListArgs) -> miette::Result<()> {
    if args.json {
        print_tasks_json(&workspace);
        return Ok(());
    }

    let explicit_environment = args
        .environment
        .map(|n| EnvironmentName::from_str(n.as_str()))
        .transpose()?
        .map(|n| {
            workspace
                .environment(&n)
                .ok_or_else(|| miette::miette!("unknown environment '{n}'"))
        })
        .transpose()?;

    let lockfile = workspace.load_lock_file().await.ok();

    let env_task_map: HashMap<Environment, HashSet<TaskName>> =
        if let Some(explicit_environment) = explicit_environment {
            HashMap::from([(
                explicit_environment.clone(),
                explicit_environment.get_filtered_tasks(),
            )])
        } else {
            workspace
                .environments()
                .iter()
                .filter_map(|env| {
                    if verify_current_platform_can_run_environment(env, lockfile.as_ref()).is_ok() {
                        Some((env.clone(), env.get_filtered_tasks()))
                    } else {
                        None
                    }
                })
                .collect()
        };

    let available_tasks: HashSet<TaskName> = env_task_map.values().flatten().cloned().collect();

    if available_tasks.is_empty() {
        eprintln!("No tasks found",);
        return Ok(());
    }

    if args.machine_readable {
        let unformatted: String = available_tasks
            .iter()
            .sorted()
            .map(|name| name.as_str())
            .join(" ");
        println!("{}", unformatted);
        return Ok(());
    }

    let tasks_per_env = env_task_map
        .into_iter()
        .map(|(env, task_names)| {
            let task_map = task_names
                .into_iter()
                .flat_map(|task_name| {
                    env.task(&task_name, Some(env.best_platform()))
                        .ok()
                        .map(|task| (task_name, task))
                })
                .collect();
            (env, task_map)
        })
        .collect();

    print_tasks(tasks_per_env, args.summary).into_diagnostic()?;
    Ok(())
}

async fn alias_task(mut workspace: WorkspaceMut, args: AliasArgs) -> miette::Result<()> {
    let name = &args.alias;
    let task: Task = args.clone().into();
    workspace.manifest().add_task(
        name.clone(),
        task.clone(),
        args.platform,
        &FeatureName::DEFAULT,
    )?;
    workspace.save().await.into_diagnostic()?;
    eprintln!(
        "{} Added alias `{}`: {}",
        console::style("@").blue(),
        name.fancy_display().bold(),
        task,
    );
    Ok(())
}

async fn remove_tasks(mut workspace: WorkspaceMut, args: RemoveArgs) -> miette::Result<()> {
    let mut to_remove = Vec::new();
    let feature = args
        .feature
        .map_or_else(FeatureName::default, FeatureName::from);
    for name in args.names.iter() {
        if let Some(platform) = args.platform {
            if !workspace
                .workspace()
                .workspace
                .value
                .tasks(Some(platform), &feature)?
                .contains_key(name)
            {
                eprintln!(
                    "{}Task '{}' does not exist on {}",
                    console::style(console::Emoji("❌ ", "X")).red(),
                    name.fancy_display().bold(),
                    console::style(platform.as_str()).bold(),
                );
                continue;
            }
        } else if !workspace
            .workspace()
            .workspace
            .value
            .tasks(None, &feature)?
            .contains_key(name)
        {
            eprintln!(
                "{}Task `{}` does not exist for the `{}` feature",
                console::style(console::Emoji("❌ ", "X")).red(),
                name.fancy_display().bold(),
                console::style(&feature).bold(),
            );
            continue;
        }

        // Safe to remove
        to_remove.push((name, args.platform));
    }

    let mut removed = Vec::with_capacity(to_remove.len());
    for (name, platform) in to_remove {
        workspace
            .manifest()
            .remove_task(name.clone(), platform, &feature)?;
        removed.push(name);
    }

    workspace.save().await.into_diagnostic()?;

    for name in removed {
        eprintln!(
            "{}Removed task `{}` ",
            console::style(console::Emoji("✔ ", "+")).green(),
            name.fancy_display().bold(),
        );
    }

    Ok(())
}

async fn add_task(mut workspace: WorkspaceMut, args: AddArgs) -> miette::Result<()> {
    let name = &args.name;
    let task: Task = args.clone().into();
    let feature = args
        .feature
        .map_or_else(FeatureName::default, FeatureName::from);
    workspace
        .manifest()
        .add_task(name.clone(), task.clone(), args.platform, &feature)?;
    workspace.save().await.into_diagnostic()?;
    eprintln!(
        "{}Added task `{}`: {}",
        console::style(console::Emoji("✔ ", "+")).green(),
        name.fancy_display().bold(),
        task,
    );
    Ok(())
}

fn print_tasks_json(project: &Workspace) {
    let env_feature_task_map: Vec<EnvTasks> = build_env_feature_task_map(project);

    let json_string =
        serde_json::to_string_pretty(&env_feature_task_map).expect("Failed to serialize tasks");
    println!("{}", json_string);
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
