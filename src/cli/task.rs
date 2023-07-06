use crate::task::{Alias, CmdArgs, Execute, Task};
use crate::Project;
use clap::Parser;
use itertools::Itertools;
use std::path::PathBuf;

#[derive(Parser, Debug)]
pub enum Operation {
    /// Add a command to the project
    Add(AddArgs),

    /// Remove a command from the project
    Remove(RemoveArgs),

    /// Alias another specific command
    Alias(AliasArgs),
}

#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct RemoveArgs {
    /// Task name
    pub name: String,
}

#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct AddArgs {
    /// Task name
    pub name: String,

    /// One or more commands to actually execute
    #[clap(required = true, num_args = 1..)]
    pub commands: Vec<String>,

    /// Depends on these other commands
    #[clap(long)]
    #[clap(num_args = 1..)]
    pub depends_on: Option<Vec<String>>,
}

#[derive(Parser, Debug)]
#[clap(arg_required_else_help = true)]
pub struct AliasArgs {
    /// Alias name
    pub alias: String,

    /// Depends on these tasks to execute
    #[clap(required = true, num_args = 1..)]
    pub depends_on: Vec<String>,
}

impl From<AddArgs> for Task {
    fn from(value: AddArgs) -> Self {
        let depends_on = value.depends_on.unwrap_or_default();

        if depends_on.is_empty() {
            Self::Plain(if value.commands.len() == 1 {
                value.commands[0].clone()
            } else {
                shlex::join(value.commands.iter().map(AsRef::as_ref))
            })
        } else {
            Self::Execute(Execute {
                cmd: CmdArgs::Single(if value.commands.len() == 1 {
                    value.commands[0].clone()
                } else {
                    shlex::join(value.commands.iter().map(AsRef::as_ref))
                }),
                depends_on,
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

pub fn execute(args: Args) -> anyhow::Result<()> {
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    match args.operation {
        Operation::Add(args) => {
            let name = args.name.clone();
            let task: Task = args.into();
            project.add_task(&name, task.clone())?;
            eprintln!(
                "{}Added task {}: {}",
                console::style(console::Emoji("✔ ", "+")).green(),
                console::style(&name).bold(),
                task,
            );
        }
        Operation::Remove(args) => {
            let name = args.name;
            project.remove_task(&name)?;
            let depends_on = project.task_depends_on(&name);
            if !depends_on.is_empty() {
                eprintln!(
                    "{}: {}",
                    console::style("Warning, the following task/s depend on this task").yellow(),
                    console::style(depends_on.iter().to_owned().join(", ")).bold()
                );
                eprintln!(
                    "{}",
                    console::style("Be sure to modify these after the removal\n").yellow()
                );
            }

            eprintln!(
                "{}Removed task {} ",
                console::style(console::Emoji("❌ ", "X")).yellow(),
                console::style(&name).bold(),
            );
        }
        Operation::Alias(args) => {
            let name = args.alias.clone();
            let task: Task = args.into();
            project.add_task(&name, task.clone())?;
            eprintln!(
                "{} Added alias {}: {}",
                console::style("@").blue(),
                console::style(&name).bold(),
                task,
            );
        }
    };

    Ok(())
}
