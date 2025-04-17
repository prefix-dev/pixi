use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    fmt::{Display, Formatter},
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::workspace::JINJA_ENV;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{Diagnostic, SourceSpan};
use serde::Serialize;
use thiserror::Error;
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

    /// If this command is an execute command, returns the `Execute` task.
    pub fn as_execute(&self) -> Result<&Execute, miette::Report> {
        match self {
            Task::Execute(execute) => Ok(execute),
            _ => Err(miette::miette!("Task is not an execute task")),
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
    pub fn as_single_command(
        &self,
        args_values: Option<&ArgValues>,
    ) -> Result<Option<Cow<str>>, TaskStringError> {
        match self {
            Task::Plain(str) => match str.render(args_values) {
                Ok(rendered) => Ok(Some(Cow::Owned(rendered))),
                Err(e) => Err(e),
            },
            Task::Custom(custom) => custom.cmd.as_single(args_values),
            Task::Execute(exe) => exe.cmd.as_single(args_values),
            Task::Alias(_) => Ok(None),
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
    pub fn get_args(&self) -> Option<&Vec<TaskArg>> {
        match self {
            Task::Execute(exe) => exe.args.as_ref(),
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
    pub args: Option<Vec<TaskArg>>,
}

impl From<Execute> for Task {
    fn from(value: Execute) -> Self {
        Task::Execute(Box::new(value))
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize)]
pub struct ArgName(String);

impl FromStr for ArgName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains('-') {
            Err(format!(
                "'{s}' is not a valid argument name since it contains the character '-'"
            ))
        } else {
            Ok(ArgName(s.to_string()))
        }
    }
}

impl ArgName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize)]
pub struct TaskArg {
    /// The name of the argument
    pub name: ArgName,

    /// The default value of the argument
    pub default: Option<String>,
}

impl std::str::FromStr for TaskArg {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(TaskArg {
            name: ArgName::from_str(s)?,
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
pub struct TaskString(String);

impl From<&str> for TaskString {
    fn from(value: &str) -> Self {
        TaskString(value.to_string())
    }
}

impl From<String> for TaskString {
    fn from(value: String) -> Self {
        TaskString(value)
    }
}

impl TaskString {
    pub fn new(value: String) -> Self {
        TaskString(value)
    }
}

/// Represents the arguments to pass to a task
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub enum ArgValues {
    FreeFormArgs(Vec<String>),
    TypedArgs(Vec<TypedArg>),
}

impl ArgValues {
    pub fn is_empty(&self) -> bool {
        match self {
            ArgValues::FreeFormArgs(args) => args.is_empty(),
            ArgValues::TypedArgs(args) => args.is_empty(),
        }
    }
}

impl Default for ArgValues {
    fn default() -> Self {
        Self::FreeFormArgs(Vec::new())
    }
}

#[derive(Debug, Diagnostic, Error, Clone)]
#[error("failed to replace argument placeholders")]
pub struct TaskStringError {
    #[source_code]
    src: String,
    #[label = "this part can't be replaced"]
    err_span: SourceSpan,
}

impl TaskStringError {
    pub fn get_source(&self) -> &str {
        &self.src
    }
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct TypedArg {
    pub name: String,
    pub value: String,
}

impl TaskString {
    pub fn render(&self, args: Option<&ArgValues>) -> Result<String, TaskStringError> {
        let context = if let Some(ArgValues::TypedArgs(args)) = args {
            let args_map: HashMap<&str, &str> = args
                .iter()
                .map(|arg| (arg.name.as_str(), arg.value.as_str()))
                .collect();
            minijinja::Value::from_serialize(&args_map)
        } else {
            minijinja::Value::default()
        };

        JINJA_ENV
            .render_str(&self.0, context)
            .map_err(|e| TaskStringError {
                src: self.0.clone(),
                err_span: e.range().unwrap_or_default().into(),
            })
    }

    pub fn source(&self) -> &str {
        &self.0
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
        args_values: Option<&ArgValues>,
    ) -> Result<Option<Cow<str>>, TaskStringError> {
        match self {
            CmdArgs::Single(cmd) => Ok(Some(Cow::Owned(cmd.render(args_values)?))),
            CmdArgs::Multiple(args) => {
                let mut rendered_args = Vec::new();
                for arg in args {
                    let rendered = arg.render(args_values)?;
                    rendered_args.push(quote(&rendered).to_string());
                }
                Ok(Some(Cow::Owned(rendered_args.join(" "))))
            }
        }
    }

    /// Returns a single string representation of the command arguments.
    pub fn into_single(
        self,
        args_values: Option<&ArgValues>,
    ) -> Result<Option<String>, TaskStringError> {
        match self {
            CmdArgs::Single(cmd) => cmd.render(args_values).map(Some),
            CmdArgs::Multiple(args) => {
                let rendered_args = args
                    .iter()
                    .map(|arg| {
                        Ok(match arg.render(args_values) {
                            Ok(rendered) => quote(&rendered).to_string(),
                            Err(e) => return Err(e),
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some(rendered_args.join(" ")))
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
                write!(f, "{}", cmd.source())?;
            }
            Task::Execute(execute) => match &execute.cmd {
                CmdArgs::Single(cmd) => write!(f, "{}", cmd.source())?,
                CmdArgs::Multiple(mult) => {
                    write!(f, "{}", mult.iter().map(|arg| arg.source()).join(" "))?
                }
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
            Task::Plain(str) => Item::Value(str.source().into()),
            Task::Execute(process) => {
                let mut table = Table::new().into_inline_table();
                match &process.cmd {
                    CmdArgs::Single(cmd_str) => {
                        table.insert("cmd", cmd_str.source().into());
                    }
                    CmdArgs::Multiple(cmd_strs) => {
                        table.insert(
                            "cmd",
                            Value::Array(Array::from_iter(cmd_strs.iter().map(|arg| arg.source()))),
                        );
                    }
                }
                if !process.depends_on.is_empty() {
                    table.insert(
                        "depends-on",
                        Value::Array(Array::from_iter(process.depends_on.iter().map(|dep| {
                            match &dep.args {
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
                            }
                        }))),
                    );
                }
                if let Some(cwd) = &process.cwd {
                    table.insert("cwd", cwd.to_string_lossy().to_string().into());
                }
                if let Some(env) = &process.env {
                    table.insert("env", Value::InlineTable(env.into_iter().collect()));
                }
                if let Some(description) = &process.description {
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
