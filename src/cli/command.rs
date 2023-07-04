use crate::command::{AliasCmd, CmdArgs, ProcessCmd};
use crate::Project;
use clap::Parser;
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
pub struct RemoveArgs {
    /// Command name
    pub name: String,
}

#[derive(Parser, Debug)]
pub struct AddArgs {
    /// Command name
    pub name: String,

    /// One or more commands to actually execute
    pub commands: Vec<String>,

    /// Depends on these other commands
    #[clap(long)]
    pub depends_on: Option<Vec<String>>,
}

#[derive(Parser, Debug)]
pub struct AliasArgs {
    /// Alias name
    pub alias: String,

    /// Depends on these commands to execute
    pub depends_on: Vec<String>,
}

impl From<AddArgs> for crate::command::Command {
    fn from(value: AddArgs) -> Self {
        let first_command = value.commands.get(0).cloned().unwrap_or_default();
        let depends_on = value.depends_on.unwrap_or_default();

        if value.commands.len() < 2 && depends_on.is_empty() {
            return Self::Plain(first_command);
        }

        Self::Process(ProcessCmd {
            cmd: if value.commands.len() == 1 {
                CmdArgs::Single(first_command)
            } else {
                CmdArgs::Multiple(value.commands)
            },
            depends_on,
        })
    }
}

impl From<AliasArgs> for crate::command::Command {
    fn from(value: AliasArgs) -> Self {
        Self::Alias(AliasCmd {
            depends_on: value.depends_on,
        })
    }
}

/// Command management in project
#[derive(Parser, Debug)]
#[clap(trailing_var_arg = true, arg_required_else_help = true)]
pub struct Args {
    /// Add, remove, or update a command
    #[clap(subcommand)]
    pub operation: Operation,

    /// The path to 'pixi.toml'
    #[arg(long)]
    pub manifest_path: Option<PathBuf>,
}

pub fn execute(args: Args) -> anyhow::Result<()> {
    let mut project = Project::load_or_else_discover(args.manifest_path.as_deref())?;
    let name = match args.operation {
        Operation::Add(args) => {
            let name = args.name.clone();
            project.add_command(&name, args.into())?;
            name
        }
        Operation::Remove(args) => {
            project.remove_command(&args.name)?;
            args.name
        }
        Operation::Alias(args) => {
            let name = args.alias.clone();
            project.add_command(&name, args.into())?;
            name
        }
    };

    eprintln!(
        "{}Added command {}",
        console::style(console::Emoji("✔ ", "")).green(),
        &name,
    );
    Ok(())
}
