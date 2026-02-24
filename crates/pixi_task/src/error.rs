use std::fmt::{Display, Formatter};

use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_consts::consts;
use thiserror::Error;

use pixi_manifest::{EnvironmentName, TaskName};

#[derive(Debug, Error, Diagnostic)]
#[error("could not find the task '{0}'", task_name.fancy_display())]
pub struct MissingTaskError {
    pub task_name: TaskName,
}

// TODO: We should make this error much better
#[derive(Debug, Error)]
pub struct AmbiguousTaskError {
    pub task_name: TaskName,
    pub environments: Vec<EnvironmentName>,
}

impl Display for AmbiguousTaskError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "the task '{}' is ambiguous", &self.task_name)
    }
}

impl Diagnostic for AmbiguousTaskError {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        Some(Box::new(format!(
            "These environments provide the task '{task_name}': {}\n\nSpecify the '--environment' flag to run the task in a specific environment, e.g:.\n\n\t{} run --environment {} {task_name}",
            self.environments
                .iter()
                .map(|env| env.as_str())
                .format(", "),
            env!("CARGO_PKG_NAME"),
            self.environments
                .first()
                .expect("there should be at least two environment"),
            task_name = &self.task_name
        )))
    }
}

#[derive(Debug, Error)]
pub struct MissingArgError {
    pub arg: String,
    pub task: String,
    pub choices: Option<String>,
}

impl Display for MissingArgError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no value provided for argument '{}' of task '{}'",
            consts::TASK_STYLE.apply_to(&self.arg),
            consts::TASK_STYLE.apply_to(&self.task),
        )?;
        if let Some(choices) = &self.choices {
            write!(f, ", choose from: {}", consts::TASK_STYLE.apply_to(choices))?;
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub struct InvalidArgValueError {
    pub arg: String,
    pub task: String,
    pub value: String,
    pub choices: String,
}

impl Display for InvalidArgValueError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "got '{}' for argument '{}' of task '{}', choose from: {}",
            consts::TASK_ERROR_STYLE.apply_to(&self.value),
            consts::TASK_STYLE.apply_to(&self.arg),
            consts::TASK_STYLE.apply_to(&self.task),
            consts::TASK_STYLE.apply_to(&self.choices),
        )
    }
}
