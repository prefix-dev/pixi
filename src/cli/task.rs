use crate::cli::cli_config::ProjectConfig;
use crate::project::virtual_packages::verify_current_platform_has_required_virtual_packages;
use crate::project::Environment;
use crate::Workspace;
use clap::Parser;
use fancy_display::FancyDisplay;
use indexmap::IndexMap;
use itertools::Itertools;
use pixi_manifest::task::{quote, Alias, CmdArgs, Execute, Task, TaskName};
use pixi_manifest::EnvironmentName;
use pixi_manifest::FeatureName;
use rattler_conda_types::Platform;
use serde::Serialize;
use serde_with::serde_as;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Parser, Debug)]
pub enum Operation {
    /// Add a command to the project
    #[clap(visible_alias = "a")]
    Add(AddArgs),

    /// Remove a command from the project
    #[clap(visible_alias = "rm")]
    Remove(RemoveArgs),

    /// Alias another specific command
    #[clap(alias = "@")]
    Alias(AliasArgs),

    /// List all tasks in the project
    #[clap(visible_alias = "ls", alias = "l")]
    List(ListArgs),
}

#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct RemoveArgs {
    /// Task names to remove
    pub names: Vec<TaskName>,

    /// The platform for which the task should be removed
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The feature for which the task should be removed
    #[arg(long, short)]
    pub feature: Option<String>,
}

#[derive(Parser, Debug, Clone)]
#[clap(arg_required_else_help = true)]
pub struct AddArgs {
    /// Task name
    pub name: TaskName,

    /// One or more commands to actually execute
    #[clap(required = true, num_args = 1..)]
    pub commands: Vec<String>,

    /// Depends on these other commands
    #[clap(long)]
    #[clap(num_args = 1..)]
    pub depends_on: Option<Vec<TaskName>>,

    /// The platform for which the task should be added
    #[arg(long, short)]
    pub platform: Option<Platform>,

    /// The feature for which the task should be added
    #[arg(long, short)]
    pub feature: Option<String>,

    /// The working directory relative to the root of the project
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// The environment variable to set, use --env key=value multiple times for more than one variable
    #[arg(long, value_parser = parse_key_val)]
    pub env: Vec<(String, String)>,

    /// A description of the task to be added.
    #[arg(long)]
    pub description: Option<String>,

    /// Isolate the task from the shell environment, and only use the pixi environment to run the task
    #[arg(long)]
    pub clean_env: bool,
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
    pub depends_on: Vec<TaskName>,

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
        let cmd_args = if value.commands.len() == 1 {
            value.commands.into_iter().next().unwrap()
        } else {
            // Simply concatenate all arguments
            value
                .commands
                .into_iter()
                .map(|arg| quote(&arg).into_owned())
                .join(" ")
        };

        // Depending on whether the task has a command, and depends_on or not we create a plain or
        // complex, or alias command.
        if cmd_args.trim().is_empty() && !depends_on.is_empty() {
            Self::Alias(Alias {
                depends_on,
                description,
            })
        } else if depends_on.is_empty()
            && value.cwd.is_none()
            && value.env.is_empty()
            && description.is_none()
        {
            Self::Plain(cmd_args)
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

            Self::Execute(Execute {
                cmd: CmdArgs::Single(cmd_args),
                depends_on,
                inputs: None,
                outputs: None,
                cwd,
                env,
                description,
                clean_env,
            })
        }
    }
}

impl From<AliasArgs> for Task {
    fn from(value: AliasArgs) -> Self {
        Self::Alias(Alias {
            depends_on: value.depends_on,
            description: value.description,
        })
    }
}

/// Interact with tasks in the project
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// Add, remove, or update a task
    #[clap(subcommand)]
    pub operation: Operation,

    #[clap(flatten)]
    pub project_config: ProjectConfig,
}

fn print_heading(value: &str) {
    let bold = console::Style::new().bold();
    eprintln!("{}\n{:-<2$}", bold.apply_to(value), "", value.len(),);
}

fn list_tasks(
    task_map: HashMap<Environment, HashMap<TaskName, Task>>,
    summary: bool,
) -> io::Result<()> {
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
                    format!(
                        " - {:<15} {}",
                        taskname.fancy_display(),
                        console::style(description).italic()
                    ),
                );
            }
        });
    });

    print_heading("Tasks that can run on this machine:");
    let formatted_tasks: String = all_tasks.iter().map(|name| name.fancy_display()).join(", ");
    eprintln!("{}", formatted_tasks);

    let formatted_descriptions: String = formatted_descriptions.values().join("\n");
    eprintln!("\n{}", formatted_descriptions);

    Ok(())
}

pub fn execute(args: Args) -> miette::Result<()> {
    let mut project =
        Workspace::load_or_else_discover(args.project_config.manifest_path.as_deref())?;
    match args.operation {
        Operation::Add(args) => {
            let name = &args.name;
            let task: Task = args.clone().into();
            let feature = args
                .feature
                .map_or(FeatureName::Default, FeatureName::Named);
            project
                .manifest
                .add_task(name.clone(), task.clone(), args.platform, &feature)?;
            project.save()?;
            eprintln!(
                "{}Added task `{}`: {}",
                console::style(console::Emoji("✔ ", "+")).green(),
                name.fancy_display().bold(),
                task,
            );
        }
        Operation::Remove(args) => {
            let mut to_remove = Vec::new();
            let feature = args
                .feature
                .map_or(FeatureName::Default, FeatureName::Named);
            for name in args.names.iter() {
                if let Some(platform) = args.platform {
                    if !project
                        .manifest
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
                } else if !project.manifest.tasks(None, &feature)?.contains_key(name) {
                    eprintln!(
                        "{}Task `{}` does not exist for the `{}` feature",
                        console::style(console::Emoji("❌ ", "X")).red(),
                        name.fancy_display().bold(),
                        console::style(&feature).bold(),
                    );
                    continue;
                }

                // Check if task has dependencies
                // TODO: Make this properly work by inspecting which actual tasks depend on the task
                //  we just removed taking into account environments and features.
                // let depends_on = project.task_names_depending_on(name);
                // if !depends_on.is_empty() && !args.names.contains(name) {
                //     eprintln!(
                //         "{}: {}",
                //         console::style("Warning, the following task/s depend on this task")
                //             .yellow(),
                //         console::style(depends_on.iter().to_owned().join(", ")).bold()
                //     );
                //     eprintln!(
                //         "{}",
                //         console::style("Be sure to modify these after the removal\n").yellow()
                //     );
                // }

                // Safe to remove
                to_remove.push((name, args.platform));
            }

            for (name, platform) in to_remove {
                project
                    .manifest
                    .remove_task(name.clone(), platform, &feature)?;
                project.save()?;
                eprintln!(
                    "{}Removed task `{}` ",
                    console::style(console::Emoji("✔ ", "+")).green(),
                    name.fancy_display().bold(),
                );
            }
        }
        Operation::Alias(args) => {
            let name = &args.alias;
            let task: Task = args.clone().into();
            project.manifest.add_task(
                name.clone(),
                task.clone(),
                args.platform,
                &FeatureName::Default,
            )?;
            project.save()?;
            eprintln!(
                "{} Added alias `{}`: {}",
                console::style("@").blue(),
                name.fancy_display().bold(),
                task,
            );
        }
        Operation::List(args) => {
            if args.json {
                print_tasks_json(&project);
                return Ok(());
            }

            let explicit_environment = args
                .environment
                .map(|n| EnvironmentName::from_str(n.as_str()))
                .transpose()?
                .map(|n| {
                    project
                        .environment(&n)
                        .ok_or_else(|| miette::miette!("unknown environment '{n}'"))
                })
                .transpose()?;

            let env_task_map: HashMap<Environment, HashSet<TaskName>> =
                if let Some(explicit_environment) = explicit_environment {
                    HashMap::from([(
                        explicit_environment.clone(),
                        explicit_environment.get_filtered_tasks(),
                    )])
                } else {
                    project
                        .environments()
                        .iter()
                        .filter_map(|env| {
                            if verify_current_platform_has_required_virtual_packages(env).is_ok() {
                                Some((env.clone(), env.get_filtered_tasks()))
                            } else {
                                None
                            }
                        })
                        .collect()
                };

            let available_tasks: HashSet<TaskName> =
                env_task_map.values().flatten().cloned().collect();

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
                    let tasks: HashMap<TaskName, Task> = task_names
                        .into_iter()
                        .filter_map(|task_name| {
                            env.task(&task_name, Some(env.best_platform()))
                                .ok()
                                .map(|task| (task_name, task.clone()))
                        })
                        .collect();
                    (env, tasks)
                })
                .collect();

            list_tasks(tasks_per_env, args.summary).expect("io error when printing tasks");
        }
    };

    Workspace::warn_on_discovered_from_env(args.project_config.manifest_path.as_deref());
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
        .filter_map(|env: &Environment<'_>| {
            if verify_current_platform_has_required_virtual_packages(env).is_err() {
                return None;
            }
            Some(EnvTasks::from(env))
        })
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
    depends_on: Vec<TaskName>,
    cwd: Option<PathBuf>,
    env: Option<IndexMap<String, String>>,
    clean_env: bool,
    inputs: Option<Vec<String>>,
    outputs: Option<Vec<String>>,
}

impl From<&Task> for TaskInfo {
    fn from(task: &Task) -> Self {
        TaskInfo {
            cmd: task.as_single_command().map(|cmd| cmd.to_string()),
            description: task.description().map(|desc| desc.to_string()),
            depends_on: task.depends_on().to_vec(),
            cwd: task.working_directory().map(PathBuf::from),
            env: task.env().cloned(),
            clean_env: task.clean_env(),
            inputs: task
                .inputs()
                .map(|inputs| inputs.iter().map(String::from).collect()),
            outputs: task
                .outputs()
                .map(|outputs| outputs.iter().map(String::from).collect()),
        }
    }
}
