use indexmap::IndexMap;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_with::{formats::PreferMany, serde_as, OneOrMany};
use std::borrow::Cow;
use std::fmt::{Display, Formatter};
use std::path::{Path, PathBuf};
use toml_edit::{Array, Item, Table, Value};

/// Represents a task name
#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct TaskName(String);

impl TaskName {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for TaskName {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl From<&str> for TaskName {
    fn from(name: &str) -> Self {
        TaskName(name.to_string())
    }
}
impl From<String> for TaskName {
    fn from(name: String) -> Self {
        TaskName(name)
    }
}
impl From<TaskName> for String {
    fn from(task_name: TaskName) -> Self {
        task_name.0 // Assuming TaskName is a tuple struct with the first element as String
    }
}

/// Represents different types of scripts
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Task {
    Plain(String),
    Execute(Execute),
    Alias(Alias),
    // We want a way for the deserializer to except a custom task, as they are meant for tasks
    // given in the command line.
    #[serde(skip)]
    Custom(Custom),
}

impl Task {
    /// Returns the names of the task that this task depends on
    pub fn depends_on(&self) -> &[TaskName] {
        match self {
            Task::Plain(_) | Task::Custom(_) => &[],
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

    /// If this command is an execute command, returns the `Execute` task.
    pub fn as_execute(&self) -> Option<&Execute> {
        match self {
            Task::Execute(execute) => Some(execute),
            _ => None,
        }
    }

    /// If this command is an alias, returns the `Alias` task.
    pub fn as_alias(&self) -> Option<&Alias> {
        match self {
            Task::Alias(alias) => Some(alias),
            _ => None,
        }
    }

    /// Returns true if this task is directly executable
    pub fn is_executable(&self) -> bool {
        match self {
            Task::Plain(_) | Task::Custom(_) | Task::Execute(_) => true,
            Task::Alias(_) => false,
        }
    }

    /// Returns the command to execute.
    pub fn as_command(&self) -> Option<CmdArgs> {
        match self {
            Task::Plain(str) => Some(CmdArgs::Single(str.clone())),
            Task::Custom(custom) => Some(custom.cmd.clone()),
            Task::Execute(exe) => Some(exe.cmd.clone()),
            Task::Alias(_) => None,
        }
    }

    /// Returns the command to execute as a single string.
    pub fn as_single_command(&self) -> Option<Cow<str>> {
        match self {
            Task::Plain(str) => Some(Cow::Borrowed(str)),
            Task::Custom(custom) => Some(custom.cmd.as_single()),
            Task::Execute(exe) => Some(exe.cmd.as_single()),
            Task::Alias(_) => None,
        }
    }

    /// Returns the environment variables for the task to run in.
    pub fn env(&self) -> Option<&IndexMap<String, String>> {
        match self {
            Task::Plain(_) => None,
            Task::Custom(_) => None,
            Task::Execute(exe) => exe.env.as_ref(),
            Task::Alias(_) => None,
        }
    }

    /// Returns the working directory for the task to run in.
    pub fn working_directory(&self) -> Option<&Path> {
        match self {
            Task::Plain(_) => None,
            Task::Custom(custom) => custom.cwd.as_deref(),
            Task::Execute(exe) => exe.cwd.as_deref(),
            Task::Alias(_) => None,
        }
    }

    /// Returns the description of the task.
    pub fn description(&self) -> Option<&str> {
        match self {
            Task::Plain(_) => None,
            Task::Custom(_) => None,
            Task::Execute(exe) => exe.description.as_deref(),
            Task::Alias(_) => None,
        }
    }

    /// True if this task is a custom task instead of something defined in a project.
    pub fn is_custom(&self) -> bool {
        matches!(self, Task::Custom(_))
    }

    /// True if this task is meant to run in a clean environment, stripped of all non required variables.
    pub fn clean_env(&self) -> bool {
        match self {
            Task::Plain(_) => false,
            Task::Custom(_) => false,
            Task::Execute(execute) => execute.clean_env,
            Task::Alias(_) => false,
        }
    }
}

/// A command script executes a single command from the environment
#[serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Execute {
    /// A list of arguments, the first argument denotes the command to run. When deserializing both
    /// an array of strings and a single string are supported.
    pub cmd: CmdArgs,

    /// A list of glob patterns that should be watched for changes before this command is run
    pub inputs: Option<Vec<String>>,

    /// A list of glob patterns that are generated by this command
    pub outputs: Option<Vec<String>>,

    /// A list of commands that should be run before this one
    // BREAK: Make the remove the alias and force kebab-case
    #[serde(default, alias = "depends_on")]
    #[serde_as(deserialize_as = "OneOrMany<_, PreferMany>")]
    pub depends_on: Vec<TaskName>,

    /// The working directory for the command relative to the root of the project.
    pub cwd: Option<PathBuf>,

    /// A list of environment variables to set before running the command
    pub env: Option<IndexMap<String, String>>,

    /// A description of the task
    pub description: Option<String>,

    /// Isolate the task from the running machine
    #[serde(default)]
    pub clean_env: bool,
}

impl From<Execute> for Task {
    fn from(value: Execute) -> Self {
        Task::Execute(value)
    }
}

/// A custom command script executes a single command in the environment
#[derive(Debug, Clone)]
pub struct Custom {
    /// A list of arguments, the first argument denotes the command to run. When deserializing both
    /// an array of strings and a single string are supported.
    pub cmd: CmdArgs,

    /// The working directory for the command relative to the root of the project.
    pub cwd: Option<PathBuf>,
}

impl From<Custom> for Task {
    fn from(value: Custom) -> Self {
        Task::Custom(value)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CmdArgs {
    Single(String),
    Multiple(Vec<String>),
}

impl From<Vec<String>> for CmdArgs {
    fn from(value: Vec<String>) -> Self {
        CmdArgs::Multiple(value)
    }
}

impl From<String> for CmdArgs {
    fn from(value: String) -> Self {
        CmdArgs::Single(value)
    }
}

impl CmdArgs {
    /// Returns a single string representation of the command arguments.
    pub fn as_single(&self) -> Cow<str> {
        match self {
            CmdArgs::Single(cmd) => Cow::Borrowed(cmd),
            CmdArgs::Multiple(args) => Cow::Owned(args.iter().map(|arg| quote(arg)).join(" ")),
        }
    }

    /// Returns a single string representation of the command arguments.
    pub fn into_single(self) -> String {
        match self {
            CmdArgs::Single(cmd) => cmd,
            CmdArgs::Multiple(args) => args.iter().map(|arg| quote(arg)).join(" "),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde_as]
pub struct Alias {
    /// A list of commands that should be run before this one
    #[serde(alias = "depends-on")]
    #[serde_as(deserialize_as = "OneOrMany<_, PreferMany>")]
    pub depends_on: Vec<TaskName>,

    /// A description of the task.
    pub description: Option<String>,
}

impl Display for Task {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Task::Plain(cmd) => {
                write!(f, "{}", cmd)?;
            }
            Task::Execute(cmd) => match &cmd.cmd {
                CmdArgs::Single(cmd) => write!(f, "{}", cmd)?,
                CmdArgs::Multiple(mult) => write!(f, "{}", mult.join(" "))?,
            },
            _ => {}
        };

        let depends_on = self.depends_on();
        if !depends_on.is_empty() {
            if depends_on.len() == 1 {
                write!(f, ", depends-on = '{}'", depends_on.iter().format(","))?;
            } else {
                write!(f, ", depends-on = [{}]", depends_on.iter().format(","))?;
            }
        }

        let env = self.env();
        if let Some(env) = env {
            if !env.is_empty() {
                write!(f, ", env = {:?}", env)?;
            }
        }
        let description = self.description();
        if let Some(description) = description {
            write!(f, ", description = {:?}", description)?;
        }

        Ok(())
    }
}

/// Quotes a string argument if it requires quotes to be able to be properly represented in our
/// shell implementation.
pub fn quote(in_str: &str) -> Cow<str> {
    if in_str.is_empty() {
        "\"\"".into()
    } else if in_str.contains(['\t', '\r', '\n', ' ', '[', ']']) {
        let mut out: String = String::with_capacity(in_str.len() + 2);
        out.push('"');
        for c in in_str.chars() {
            match c {
                '"' | '\\' => out.push('\\'),
                _ => (),
            }
            out.push(c);
        }
        out.push('"');
        out.into()
    } else {
        in_str.into()
    }
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
                        "depends-on",
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
                if let Some(env) = process.env {
                    table.insert("env", Value::InlineTable(env.into_iter().collect()));
                }
                if let Some(description) = process.description {
                    table.insert("description", description.into());
                }
                Item::Value(Value::InlineTable(table))
            }
            Task::Alias(alias) => {
                let mut table = Table::new().into_inline_table();
                table.insert(
                    "depends-on",
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

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn test_quote() {
        assert_eq!(quote("foobar"), "foobar");
        assert_eq!(quote("foo bar"), "\"foo bar\"");
        assert_eq!(quote("\""), "\"");
        assert_eq!(quote("foo \" bar"), "\"foo \\\" bar\"");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("$PATH"), "$PATH");
        assert_eq!(
            quote("PATH=\"$PATH;build/Debug\""),
            "PATH=\"$PATH;build/Debug\""
        );
        assert_eq!(quote("name=[64,64]"), "\"name=[64,64]\"");
    }
}
