use std::{
    borrow::Cow,
    convert::Infallible,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::workspace::JINJA_ENV;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::IntoDiagnostic;
use serde::Serialize;
use toml_edit::{Array, Item, Table, Value};

use crate::EnvironmentName;

/// Represents a task name
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash, Ord, PartialOrd)]
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

/// A task dependency with optional args
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct Dependency {
    pub task_name: TaskName,
    pub args: Option<Vec<String>>,
    pub environment: Option<EnvironmentName>,
}

impl Dependency {
    pub fn new(s: &str, args: Option<Vec<String>>, environment: Option<EnvironmentName>) -> Self {
        Dependency {
            task_name: TaskName(s.to_string()),
            args,
            environment,
        }
    }

    pub fn new_without_env(s: &str, args: Option<Vec<String>>) -> Self {
        Dependency {
            task_name: TaskName(s.to_string()),
            args,
            environment: None,
        }
    }
}

impl From<&str> for Dependency {
    fn from(s: &str) -> Self {
        Dependency::new_without_env(s, None)
    }
}

impl std::fmt::Display for Dependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.args {
            Some(args) if !args.is_empty() => write!(f, "{} with args", self.task_name),
            _ => write!(f, "{}", self.task_name),
        }
    }
}

impl FromStr for TaskName {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(TaskName(s.to_string()))
    }
}

/// Represents different types of scripts
#[derive(Debug, Clone)]
pub enum Task {
    Plain(TaskString),
    Execute(Box<Execute>),
    Alias(Alias),
    Custom(Custom),
}

impl Task {
    /// Returns the names of the task that this task depends on
    pub fn depends_on(&self) -> &[Dependency] {
        match self {
            Task::Plain(_) | Task::Custom(_) => &[],
            Task::Execute(cmd) => &cmd.depends_on,
            Task::Alias(cmd) => &cmd.depends_on,
        }
    }

    /// If this task is a plain task, returns the task string
    pub fn as_plain(&self) -> Result<String, miette::Error> {
        match self {
            Task::Plain(str) => str.render(None),
            _ => Err(miette::miette!("Task is not a plain task")),
        }
    }

    /// If this command is an execute command, returns the `Execute` task.
    pub fn as_execute(&self) -> Result<&Execute, miette::Error> {
        match self {
            Task::Execute(execute) => Ok(execute),
            _ => Err(miette::miette!("Task is not an execute task")),
        }
    }

    /// If this command is an alias, returns the `Alias` task.
    pub fn as_alias(&self) -> Result<&Alias, miette::Error> {
        match self {
            Task::Alias(alias) => Ok(alias),
            _ => Err(miette::miette!("Task is not an alias task")),
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
    pub fn as_single_command(&self) -> Result<Cow<str>, miette::Error> {
        let args = self.get_args();
        match self {
            Task::Plain(str) => match str.render(args) {
                Ok(rendered) => Ok(Cow::Owned(rendered)),
                Err(e) => Err(e),
            },
            Task::Custom(custom) => custom.cmd.as_single(args),
            Task::Execute(exe) => exe.cmd.as_single(args),
            Task::Alias(_) => Err(miette::miette!("Alias tasks cannot be executed directly")),
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
            Task::Alias(cmd) => cmd.description.as_deref(),
        }
    }

    /// True if this task is a custom task instead of something defined in a
    /// project.
    pub fn is_custom(&self) -> bool {
        matches!(self, Task::Custom(_))
    }

    /// True if this task is meant to run in a clean environment, stripped of
    /// all non required variables.
    pub fn clean_env(&self) -> bool {
        match self {
            Task::Plain(_) => false,
            Task::Custom(_) => false,
            Task::Execute(execute) => execute.clean_env,
            Task::Alias(_) => false,
        }
    }

    /// Returns the inputs of the task.
    pub fn inputs(&self) -> Option<&[String]> {
        match self {
            Task::Execute(exe) => exe.inputs.as_deref(),
            _ => None,
        }
    }

    /// Returns the outputs of the task.
    pub fn outputs(&self) -> Option<&[String]> {
        match self {
            Task::Execute(exe) => exe.outputs.as_deref(),
            _ => None,
        }
    }

    /// Returns the arguments of the task.
    pub fn get_args(&self) -> Option<&IndexMap<String, Option<String>>> {
        match self {
            Task::Execute(exe) => exe.args.as_ref(),
            _ => None,
        }
    }

    /// Creates a new task with updated arguments from the provided values.
    /// Returns None if the task doesn't support arguments.
    pub fn with_updated_args(&self, arg_values: &[String]) -> Option<Self> {
        match self {
            Task::Execute(exe) => {
                if arg_values.len() > exe.args.as_ref().map_or(0, |args| args.len()) {
                    tracing::warn!("Task has more arguments than provided values");
                    return None;
                }

                if let Some(args_map) = &exe.args {
                    let mut new_args = args_map.clone();
                    for ((arg_name, _), value) in args_map.iter().zip(arg_values.iter()) {
                        if let Some(arg_value) = new_args.get_mut(arg_name) {
                            *arg_value = Some(value.clone());
                        }
                    }

                    // Create a new Execute with the updated args
                    let mut new_exe = (**exe).clone();
                    new_exe.args = Some(new_args);

                    Some(Task::Execute(Box::new(new_exe)))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// A command script executes a single command from the environment
#[derive(Debug, Clone)]
pub struct Execute {
    /// A list of arguments, the first argument denotes the command to run. When
    /// deserializing both an array of strings and a single string are
    /// supported.
    pub cmd: CmdArgs,

    /// A list of glob patterns that should be watched for changes before this
    /// command is run
    pub inputs: Option<Vec<String>>,

    /// A list of glob patterns that are generated by this command
    pub outputs: Option<Vec<String>>,

    /// A list of commands that should be run before this one
    // BREAK: Make the remove the alias and force kebab-case
    pub depends_on: Vec<Dependency>,

    /// The working directory for the command relative to the root of the
    /// project.
    pub cwd: Option<PathBuf>,

    /// A list of environment variables to set before running the command
    pub env: Option<IndexMap<String, String>>,

    /// A description of the task
    pub description: Option<String>,

    /// Isolate the task from the running machine
    pub clean_env: bool,

    /// The arguments to pass to the task
    pub args: Option<IndexMap<String, Option<String>>>,
}

impl From<Execute> for Task {
    fn from(value: Execute) -> Self {
        Task::Execute(Box::new(value))
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq)]
pub struct TaskArg {
    /// The name of the argument
    pub name: String,

    /// The default value of the argument
    pub default: Option<String>,
}

impl std::str::FromStr for TaskArg {
    type Err = miette::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(TaskArg {
            name: s.to_string(),
            default: None,
        })
    }
}

/// A custom command script executes a single command in the environment
#[derive(Debug, Clone)]
pub struct Custom {
    /// A list of arguments, the first argument denotes the command to run. When
    /// deserializing both an array of strings and a single string are
    /// supported.
    pub cmd: CmdArgs,

    /// The working directory for the command relative to the root of the
    /// project.
    pub cwd: Option<PathBuf>,
}

impl From<Custom> for Task {
    fn from(value: Custom) -> Self {
        Task::Custom(value)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TaskString {
    _inner: String,
}

impl From<&str> for TaskString {
    fn from(value: &str) -> Self {
        TaskString {
            _inner: value.to_string(),
        }
    }
}

impl From<String> for TaskString {
    fn from(value: String) -> Self {
        TaskString { _inner: value }
    }
}

impl TaskString {
    // TODO: enable feature for serialize
    // TODO: check if the Minijinja Value can be used.
    pub fn render(
        &self,
        argsmap: Option<&IndexMap<String, Option<String>>>,
    ) -> Result<String, miette::Error> {
        if let Some(argsmap) = argsmap {
            // If any of the values are None, we should error
            if argsmap.values().any(|v| v.is_none()) {
                let undefined_arg = argsmap
                    .iter()
                    .find_map(|(k, v)| if v.is_none() { Some(k) } else { None })
                    .unwrap();
                return Err(anyhow::anyhow!(
                    "No value provided for argument '{}'",
                    undefined_arg
                ));
            }
            return JINJA_ENV
                .render_str(&self._inner, argsmap)
                .into_diagnostic();
        }

        JINJA_ENV.render_str(&self._inner, ()).into_diagnostic()
    }
}

#[derive(Debug, Clone)]
pub enum CmdArgs {
    Single(TaskString),
    Multiple(Vec<TaskString>),
}

impl From<Vec<TaskString>> for CmdArgs {
    fn from(value: Vec<TaskString>) -> Self {
        CmdArgs::Multiple(value)
    }
}

impl From<TaskString> for CmdArgs {
    fn from(value: TaskString) -> Self {
        CmdArgs::Single(value)
    }
}

impl CmdArgs {
    /// Returns a single string representation of the command arguments.
    pub fn as_single(
        &self,
        argsmap: Option<&IndexMap<String, Option<String>>>,
    ) -> Result<Cow<str>, miette::Error> {
        match self {
            CmdArgs::Single(cmd) => Ok(Cow::Owned(cmd.render(argsmap)?)),
            CmdArgs::Multiple(args) => {
                let mut rendered_args = Vec::new();
                for arg in args {
                    let rendered = arg.render(argsmap)?;
                    rendered_args.push(quote(&rendered).to_string());
                }
                Ok(Cow::Owned(rendered_args.join(" ")))
            }
        }
    }

    /// Returns a single string representation of the command arguments.
    pub fn into_single(
        self,
        argsmap: Option<&IndexMap<String, Option<String>>>,
    ) -> Result<String, miette::Error> {
        match self {
            CmdArgs::Single(cmd) => cmd.render(argsmap),
            CmdArgs::Multiple(args) => {
                let rendered_args = args
                    .iter()
                    .map(|arg| {
                        Ok(match arg.render(argsmap) {
                            Ok(rendered) => quote(&rendered).to_string(),
                            Err(e) => return Err(e),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rendered_args.join(" "))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Alias {
    /// A list of commands that should be run before this one
    pub depends_on: Vec<Dependency>,

    /// A description of the task.
    pub description: Option<String>,
}

impl Display for Task {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Task::Plain(cmd) => {
                write!(f, "{}", cmd.render(None).unwrap())?;
            }
            Task::Execute(execute) => match &execute.cmd {
                CmdArgs::Single(cmd) => {
                    write!(f, "{}", cmd.render(execute.args.as_ref()).unwrap())?
                }
                CmdArgs::Multiple(mult) => write!(
                    f,
                    "{}",
                    mult.iter()
                        .map(|arg| arg.render(execute.args.as_ref()).unwrap())
                        .join(" ")
                )?,
            },
            _ => {}
        };

        let depends_on = self.depends_on();
        if !depends_on.is_empty() {
            if depends_on.len() == 1 {
                write!(f, ", depends-on = '{}'", depends_on[0])?;
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

/// Quotes a string argument if it requires quotes to be able to be properly
/// represented in our shell implementation.
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
            Task::Plain(str) => Item::Value(str.render(None).unwrap().into()),
            Task::Execute(process) => {
                let mut table = Table::new().into_inline_table();
                match process.cmd {
                    CmdArgs::Single(cmd_str) => {
                        table.insert("cmd", cmd_str.render(process.args.as_ref()).unwrap().into());
                    }
                    CmdArgs::Multiple(cmd_strs) => {
                        table.insert(
                            "cmd",
                            Value::Array(Array::from_iter(
                                cmd_strs
                                    .iter()
                                    .map(|arg| arg.render(process.args.as_ref()).unwrap()),
                            )),
                        );
                    }
                }
                if !process.depends_on.is_empty() {
                    table.insert(
                        "depends-on",
                        Value::Array(Array::from_iter(process.depends_on.into_iter().map(
                            |dep| match &dep.args {
                                Some(args) if !args.is_empty() => {
                                    let mut table = Table::new().into_inline_table();
                                    table.insert("task", dep.task_name.to_string().into());
                                    table.insert(
                                        "args",
                                        Value::Array(Array::from_iter(
                                            args.iter().map(|arg| Value::from(arg.clone())),
                                        )),
                                    );
                                    Value::InlineTable(table)
                                }
                                _ => Value::from(dep.task_name.to_string()),
                            },
                        ))),
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
                let mut array = Array::new();
                for dep in alias.depends_on.iter() {
                    let mut table = Table::new().into_inline_table();

                    table.insert("task", dep.task_name.to_string().into());

                    if let Some(args) = &dep.args {
                        table.insert(
                            "args",
                            Value::Array(Array::from_iter(
                                args.iter().map(|arg| Value::from(arg.clone())),
                            )),
                        );
                    }

                    if let Some(env) = &dep.environment {
                        table.insert("environment", env.to_string().into());
                    }

                    array.push(Value::InlineTable(table));
                }
                Item::Value(Value::Array(array))
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
