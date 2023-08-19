use itertools::Itertools;
use serde::Deserialize;
use serde_with::{formats::PreferMany, serde_as, OneOrMany};
use std::fmt::{Display, Formatter};

/// Represents different types of scripts
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Task {
    Plain(String),
    Execute(Execute),
    Alias(Alias),
}

impl Task {
    pub fn depends_on(&self) -> &[String] {
        match self {
            Task::Plain(_) => &[],
            Task::Execute(cmd) => &cmd.depends_on,
            Task::Alias(cmd) => &cmd.depends_on,
        }
    }

    /// If this task is a plain task, returns the task string
    pub fn as_plain(&self) -> Option<&String> {
        match self {
            Task::Plain(str) => Some(str),
            _ => None,
        }
    }

    // If this command is an execute command, returns the [`Execute`] task.
    pub fn as_execute(&self) -> Option<&Execute> {
        match self {
            Task::Execute(execute) => Some(execute),
            _ => None,
        }
    }

    /// If this command is an alias, returns the [`Alias`] task.
    pub fn as_alias(&self) -> Option<&Alias> {
        match self {
            Task::Alias(alias) => Some(alias),
            _ => None,
        }
    }

    /// Returns true if this task is directly executable
    pub fn is_executable(&self) -> bool {
        match self {
            Task::Plain(_) | Task::Execute(_) => true,
            Task::Alias(_) => false,
        }
    }
}

/// A command script executes a single command from the environment
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct Execute {
    /// A list of arguments, the first argument denotes the command to run. When deserializing both
    /// an array of strings and a single string are supported.
    pub cmd: CmdArgs,

    /// A list of commands that should be run before this one
    #[serde(default)]
    #[serde_as(deserialize_as = "OneOrMany<_, PreferMany>")]
    pub depends_on: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CmdArgs {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde_as]
pub struct Alias {
    /// A list of commands that should be run before this one
    #[serde_as(deserialize_as = "OneOrMany<_, PreferMany>")]
    pub depends_on: Vec<String>,
}

impl Display for Task {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Task::Plain(cmd) => {
                write!(f, "{}", cmd)?;
            }
            Task::Execute(cmd) => {
                match &cmd.cmd {
                    CmdArgs::Single(cmd) => write!(f, "{}", cmd)?,
                    CmdArgs::Multiple(mult) => write!(f, "{}", mult.join(" "))?,
                };
                if !cmd.depends_on.is_empty() {
                    write!(f, ", ")?;
                }
            }
            _ => {}
        };

        let depends_on = self.depends_on();
        if !depends_on.is_empty() {
            if depends_on.len() == 1 {
                write!(f, "depends_on = '{}'", depends_on.iter().join(","))
            } else {
                write!(f, "depends_on = [{}]", depends_on.iter().join(","))
            }
        } else {
            Ok(())
        }
    }
}
