use std::{
    borrow::Cow,
    collections::HashMap,
    convert::Infallible,
    fmt::{Display, Formatter},
    ops::Deref,
    path::{Path, PathBuf},
    str::FromStr,
};

use crate::workspace::JINJA_ENV;
use indexmap::IndexMap;
use itertools::Itertools;
use miette::{Diagnostic, SourceSpan};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use toml_edit::{Array, InlineTable, Item, Table, Value};

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
    pub args: Option<Vec<DependencyArg>>,
    pub environment: Option<EnvironmentName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum DependencyArg {
    Positional(TemplateString),
    Named(String, TemplateString),
}

impl Dependency {
    pub fn new(
        s: &str,
        args: Option<Vec<DependencyArg>>,
        environment: Option<EnvironmentName>,
    ) -> Self {
        Dependency {
            task_name: TaskName(s.to_string()),
            args,
            environment,
        }
    }

    pub fn new_without_env(s: &str, args: Option<Vec<DependencyArg>>) -> Self {
        Dependency {
            task_name: TaskName(s.to_string()),
            args,
            environment: None,
        }
    }
    pub fn render_args(
        &self,
        args: Option<&ArgValues>,
    ) -> Result<Option<Vec<TypedDependencyArg>>, TemplateStringError> {
        match &self.args {
            Some(task_args) => {
                let mut result = Vec::new();
                for arg in task_args {
                    match arg {
                        DependencyArg::Positional(val) => {
                            result.push(TypedDependencyArg::Positional(val.render(args)?));
                        }
                        DependencyArg::Named(key, val) => {
                            result.push(TypedDependencyArg::Named(key.clone(), val.render(args)?));
                        }
                    }
                }
                Ok(Some(result))
            }
            None => Ok(None),
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

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct TypedDependency {
    pub task_name: TaskName,
    pub args: Option<Vec<TypedDependencyArg>>,
    pub environment: Option<EnvironmentName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub enum TypedDependencyArg {
    Positional(String),
    Named(String, String),
}

impl TypedDependency {
    pub fn from_dependency(
        dependency: &Dependency,
        args: Option<&ArgValues>,
    ) -> Result<Self, TemplateStringError> {
        Ok(TypedDependency {
            task_name: dependency.task_name.clone(),
            args: dependency.render_args(args)?,
            environment: dependency.environment.clone(),
        })
    }
}

/// Represents different types of scripts
#[derive(Debug, Clone)]
pub enum Task {
    Plain(TemplateString),
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
    ) -> Result<Option<Cow<str>>, TemplateStringError> {
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

    pub fn as_single_command_no_render(&self) -> Result<Option<Cow<str>>, TemplateStringError> {
        match self {
            Task::Plain(str) => Ok(Some(Cow::Owned(str.source().to_string()))),
            Task::Custom(custom) => custom.cmd.as_single_no_render(),
            Task::Execute(exe) => exe.cmd.as_single_no_render(),
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
    pub fn inputs(&self) -> Option<&GlobPatterns> {
        match self {
            Task::Execute(exe) => exe.inputs.as_ref(),
            _ => None,
        }
    }

    /// Returns the outputs of the task.
    pub fn outputs(&self) -> Option<&GlobPatterns> {
        match self {
            Task::Execute(exe) => exe.outputs.as_ref(),
            _ => None,
        }
    }

    /// Returns the arguments of the task.
    pub fn args(&self) -> Option<&[TaskArg]> {
        match self {
            Task::Execute(exe) => exe.args.as_deref(),
            Task::Alias(alias) => alias.args.as_deref(),
            _ => None,
        }
    }
}

/// A list of glob patterns that can be used as input or output for a task
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Hash, Deserialize, Default)]
pub struct GlobPatterns(Vec<TemplateString>);

impl GlobPatterns {
    pub fn new(patterns: Vec<TemplateString>) -> Self {
        GlobPatterns(patterns)
    }

    /// Renders the glob patterns using the provided arguments
    pub fn render(
        &self,
        args: Option<&ArgValues>,
    ) -> Result<Vec<RenderedString>, TemplateStringError> {
        self.0
            .iter()
            .map(|i| i.render(args).map(RenderedString::from))
            .collect::<Result<Vec<RenderedString>, _>>()
    }
}

impl Deref for GlobPatterns {
    type Target = [TemplateString];

    fn deref(&self) -> &Self::Target {
        &self.0
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
    pub inputs: Option<GlobPatterns>,

    /// A list of glob patterns that are generated by this command
    pub outputs: Option<GlobPatterns>,

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

#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct TypedArg {
    pub name: String,
    pub value: String,
}

impl Display for TypedArg {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} = {}", self.name, self.value)
    }
}

/// A string that contains placeholders to be rendered using the `minijinja` templating engine.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Hash, Deserialize)]
pub struct TemplateString(String);

impl From<&str> for TemplateString {
    fn from(value: &str) -> Self {
        TemplateString(value.to_string())
    }
}

impl From<String> for TemplateString {
    fn from(value: String) -> Self {
        TemplateString(value)
    }
}

impl TemplateString {
    pub fn new(value: String) -> Self {
        TemplateString(value)
    }

    pub fn render(&self, args: Option<&ArgValues>) -> Result<String, TemplateStringError> {
        // Only perform MiniJinja rendering when typed args are provided.
        // For free-form args or no args, return the source verbatim to avoid
        // interpreting arbitrary strings as templates.
        if let Some(ArgValues::TypedArgs(args)) = args {
            let args_map: HashMap<&str, &str> = args
                .iter()
                .map(|arg| (arg.name.as_str(), arg.value.as_str()))
                .collect();
            let context = minijinja::Value::from_serialize(&args_map);

            JINJA_ENV
                .render_str(&self.0, context)
                .map_err(|e| TemplateStringError {
                    src: self.0.clone(),
                    err_span: e.range().unwrap_or_default().into(),
                })
        } else {
            Ok(self.0.clone())
        }
    }

    pub fn source(&self) -> &str {
        &self.0
    }
}

/// A rendered string where placeholders were already replaced by arguments
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Hash)]
pub struct RenderedString(String);

impl From<&str> for RenderedString {
    fn from(value: &str) -> Self {
        RenderedString(value.to_string())
    }
}

impl From<String> for RenderedString {
    fn from(value: String) -> Self {
        RenderedString(value)
    }
}

impl RenderedString {
    /// Creates a new rendered string from the given value.
    // TODO: In theory, this should check if no placeholders are left in the string
    pub fn new(value: String) -> Self {
        RenderedString(value)
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

impl Display for ArgValues {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgValues::FreeFormArgs(args) => write!(f, "{}", args.iter().join(", ")),
            ArgValues::TypedArgs(args) => write!(f, "{}", args.iter().join(", ")),
        }
    }
}

#[derive(Debug, Diagnostic, Error, Clone)]
#[error("failed to replace argument placeholders")]
pub struct TemplateStringError {
    #[source_code]
    src: String,
    #[label = "this part can't be replaced"]
    err_span: SourceSpan,
}

impl TemplateStringError {
    pub fn get_source(&self) -> &str {
        &self.src
    }
}

#[derive(Debug, Clone)]
pub enum CmdArgs {
    Single(TemplateString),
    Multiple(Vec<TemplateString>),
}

impl From<Vec<TemplateString>> for CmdArgs {
    fn from(value: Vec<TemplateString>) -> Self {
        CmdArgs::Multiple(value)
    }
}

impl From<TemplateString> for CmdArgs {
    fn from(value: TemplateString) -> Self {
        CmdArgs::Single(value)
    }
}

impl CmdArgs {
    /// Returns a single string representation of the command arguments.
    pub fn as_single(
        &self,
        args_values: Option<&ArgValues>,
    ) -> Result<Option<Cow<str>>, TemplateStringError> {
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
    ) -> Result<Option<String>, TemplateStringError> {
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

    pub fn as_single_no_render(&self) -> Result<Option<Cow<str>>, TemplateStringError> {
        match self {
            CmdArgs::Single(cmd) => Ok(Some(Cow::Owned(cmd.source().to_string()))),
            CmdArgs::Multiple(args) => Ok(Some(Cow::Owned(
                args.iter().map(|arg| arg.source().to_string()).join(" "),
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Alias {
    /// A list of commands that should be run before this one
    pub depends_on: Vec<Dependency>,

    /// A description of the task.
    pub description: Option<String>,

    /// A list of arguments to pass to the task.
    pub args: Option<Vec<TaskArg>>,
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

                if let Some(args) = &process.args {
                    let mut args_array = Array::new();
                    for arg in args {
                        if let Some(default) = &arg.default {
                            let mut arg_table = Table::new().into_inline_table();
                            arg_table.insert("arg", arg.name.as_str().into());
                            arg_table.insert("default", default.into());
                            args_array.push(Value::InlineTable(arg_table));
                        } else {
                            args_array.push(Value::String(toml_edit::Formatted::new(
                                arg.name.as_str().to_string(),
                            )));
                        }
                    }
                    table.insert("args", Value::Array(args_array));
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
                                        Value::Array(Array::from_iter(args.iter().map(|arg| {
                                            match arg {
                                                DependencyArg::Positional(val) => {
                                                    Value::from(val.source().to_string())
                                                }
                                                DependencyArg::Named(name, val) => {
                                                    let mut table = InlineTable::new();
                                                    table.insert(
                                                        name,
                                                        Value::from(val.source().to_string()),
                                                    );
                                                    Value::InlineTable(table)
                                                }
                                            }
                                        }))),
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
                if alias.args.is_some() {
                    let mut table = Table::new().into_inline_table();

                    if let Some(args_vec) = &alias.args {
                        let mut args = Vec::new();
                        for arg in args_vec {
                            if let Some(default) = &arg.default {
                                let mut arg_table = Table::new().into_inline_table();
                                arg_table.insert("arg", arg.name.as_str().into());
                                arg_table.insert("default", default.into());
                                args.push(Value::InlineTable(arg_table));
                            } else {
                                args.push(Value::String(toml_edit::Formatted::new(
                                    arg.name.as_str().to_string(),
                                )));
                            }
                        }
                        table.insert("args", Value::Array(Array::from_iter(args)));
                    }

                    let mut deps = Vec::new();
                    for dep in alias.depends_on.iter() {
                        let mut dep_table = Table::new().into_inline_table();
                        dep_table.insert("task", dep.task_name.to_string().into());

                        if let Some(args) = &dep.args {
                            dep_table.insert(
                                "args",
                                Value::Array(Array::from_iter(args.iter().map(|arg| match arg {
                                    DependencyArg::Positional(val) => {
                                        Value::from(val.source().to_string())
                                    }
                                    DependencyArg::Named(name, val) => {
                                        let mut table = InlineTable::new();
                                        table.insert(name, Value::from(val.source().to_string()));
                                        Value::InlineTable(table)
                                    }
                                }))),
                            );
                        }

                        if let Some(env) = &dep.environment {
                            dep_table.insert("environment", env.to_string().into());
                        }

                        deps.push(Value::InlineTable(dep_table));
                    }
                    table.insert("depends-on", Value::Array(Array::from_iter(deps)));

                    if let Some(description) = &alias.description {
                        table.insert("description", description.into());
                    }

                    Item::Value(Value::InlineTable(table))
                } else {
                    let mut array = Array::new();
                    for dep in alias.depends_on.iter() {
                        let mut table = Table::new().into_inline_table();
                        table.insert("task", dep.task_name.to_string().into());

                        if let Some(args) = &dep.args {
                            table.insert(
                                "args",
                                Value::Array(Array::from_iter(args.iter().map(|arg| match arg {
                                    DependencyArg::Positional(val) => {
                                        Value::from(val.source().to_string())
                                    }
                                    DependencyArg::Named(name, val) => {
                                        let mut table = InlineTable::new();
                                        table.insert(name, Value::from(val.source().to_string()));
                                        Value::InlineTable(table)
                                    }
                                }))),
                            );
                        }

                        if let Some(env) = &dep.environment {
                            table.insert("environment", env.to_string().into());
                        }

                        array.push(Value::InlineTable(table));
                    }

                    if let Some(description) = &alias.description {
                        let mut table = Table::new().into_inline_table();
                        table.insert("depends-on", Value::Array(array));
                        table.insert("description", description.into());
                        Item::Value(Value::InlineTable(table))
                    } else {
                        Item::Value(Value::Array(array))
                    }
                }
            }
            _ => Item::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use crate::task::{Alias, Dependency, DependencyArg, Task};

    use super::quote;

    use super::{ArgValues, TemplateString, TypedArg};

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

    #[test]
    fn test_table_from_dependency_args() {
        let positional_arg = DependencyArg::Positional("foo".into());
        let named_arg = DependencyArg::Named("bar".into(), "baz".into());
        let args = vec![positional_arg, named_arg];
        let dep = Dependency::new("depTask", Some(args), None);
        let alias = Alias {
            depends_on: vec![dep],
            description: None,
            args: None,
        };
        let task = Task::Alias(alias);
        let toml = toml_edit::Item::from(task);
        assert_snapshot!(toml.to_string(), @r###"[{ task = "depTask", args = ["foo", { bar = "baz" }] }]"###);
    }

    #[test]
    fn test_template_string_no_render_without_typed_args() {
        let t = TemplateString::from("echo {{ foo }}");
        // No args -> should not render, return verbatim string
        let rendered = t.render(None).expect("should not error without typed args");
        assert_eq!(rendered, "echo {{ foo }}");

        // Free-form args -> should not render
        let rendered = t
            .render(Some(&ArgValues::FreeFormArgs(vec!["bar".into()])))
            .expect("should not error with free-form args");
        assert_eq!(rendered, "echo {{ foo }}");
    }

    #[test]
    fn test_template_string_renders_with_typed_args() {
        let t = TemplateString::from("echo {{ foo }}");
        let args = ArgValues::TypedArgs(vec![TypedArg {
            name: "foo".into(),
            value: "bar".into(),
        }]);
        let rendered = t
            .render(Some(&args))
            .expect("should render with typed args");
        assert_eq!(rendered, "echo bar");
    }
}
