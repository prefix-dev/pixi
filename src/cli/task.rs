use crate::project::manifest::{EnvironmentName, FeatureName};
use crate::task::{quote, Alias, CmdArgs, Execute, Task, TaskName};
use crate::Project;
use clap::Parser;
use itertools::Itertools;
use miette::miette;
use rattler_conda_types::Platform;
use std::path::PathBuf;
use std::str::FromStr;
use toml_edit::{Array, Item, Table, Value};

#[derive(Parser, Debug)]
pub enum Operation {
    /// Add a command to the project
    #[clap(alias = "a")]
    Add(AddArgs),

    /// Remove a command from the project
    #[clap(alias = "r")]
    Remove(RemoveArgs),

    /// Alias another specific command
    #[clap(alias = "@")]
    Alias(AliasArgs),

    /// List all tasks
    #[clap(alias = "l")]
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
}

#[derive(Parser, Debug, Clone)]
pub struct ListArgs {
    #[arg(long, short)]
    pub summary: bool,

    /// The environment the list should be generated for
    /// If not specified, the default environment is used.
    #[arg(long, short)]
    pub environment: Option<String>,
}

impl From<AddArgs> for Task {
    fn from(value: AddArgs) -> Self {
        let depends_on = value.depends_on.unwrap_or_default();

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
            Self::Alias(Alias { depends_on })
        } else if depends_on.is_empty() && value.cwd.is_none() {
            Self::Plain(cmd_args)
        } else {
            Self::Execute(Execute {
                cmd: CmdArgs::Single(cmd_args),
                depends_on,
                inputs: None,
                outputs: None,
                cwd: value.cwd,
            })
        }
    }
}

impl From<AliasArgs> for Task {
    fn from(value: AliasArgs) -> Self {
        Self::Alias(Alias {
            depends_on: value.depends_on,
        })
    }
}

/// Command management in project
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// Add, remove, or update a task
    #[clap(subcommand)]
    pub operation: Operation,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

pub fn execute(args: Args) -> miette::Result<()> {
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
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
            let env = EnvironmentName::from_str(args.environment.as_deref().unwrap_or("default"))?;
            let tasks = project
                .environment(&env)
                .ok_or(miette!("Environment `{}` not found in project", env))?
                .tasks(Some(Platform::current()), true)?
                .into_keys()
                .collect_vec();
            if tasks.is_empty() {
                eprintln!("No tasks found",);
            } else {
                let formatted: String = tasks
                    .iter()
                    .sorted()
                    .map(|name| {
                        if args.summary {
                            format!("{} ", name.as_str(),)
                        } else {
                            format!("* {}\n", name.fancy_display().bold(),)
                        }
                    })
                    .collect();

                println!("{}", formatted);
            }
        }
    };

    Project::warn_on_discovered_from_env(args.manifest_path.as_deref());
    Ok(())
}

impl From<Task> for Item {
    fn from(value: Task) -> Self {
        match value {
            Task::Plain(str) => Item::Value(str.into()),
            Task::Execute(process) => {
                let mut table = Table::new().into_inline_table();
                match process.cmd {
                    CmdArgs::Single(cmd_str) => {
                        table.insert("cmd", cmd_str.into());
                    }
                    CmdArgs::Multiple(cmd_strs) => {
                        table.insert("cmd", Value::Array(Array::from_iter(cmd_strs)));
                    }
                }
                if !process.depends_on.is_empty() {
                    table.insert(
                        "depends_on",
                        Value::Array(Array::from_iter(
                            process
                                .depends_on
                                .into_iter()
                                .map(String::from)
                                .map(Value::from),
                        )),
                    );
                }
                if let Some(cwd) = process.cwd {
                    table.insert("cwd", cwd.to_string_lossy().to_string().into());
                }
                Item::Value(Value::InlineTable(table))
            }
            Task::Alias(alias) => {
                let mut table = Table::new().into_inline_table();
                table.insert(
                    "depends_on",
                    Value::Array(Array::from_iter(
                        alias
                            .depends_on
                            .into_iter()
                            .map(String::from)
                            .map(Value::from),
                    )),
                );
                Item::Value(Value::InlineTable(table))
            }
            _ => Item::None,
        }
    }
}
