use std::fmt::{Display, Formatter};

use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::Diagnostic;
use pixi_consts::consts;
use thiserror::Error;

use pixi_manifest::{EnvironmentName, PixiPlatformName, TaskName};

#[derive(Debug, Error, Diagnostic)]
#[error("could not find the task '{0}'", task_name.fancy_display())]
pub struct MissingTaskError {
    pub task_name: TaskName,
}

/// The task exists in the workspace but not where the user asked to run it:
/// the selected environment doesn't define it, or the environments that do
/// don't apply to the selected platform / this machine.
#[derive(Debug, Error)]
pub struct UnrunnableTaskError {
    pub task_name: TaskName,
    /// Environments that define the task, for any of their platforms.
    pub environments: Vec<EnvironmentName>,
    /// The environment the user selected with `--environment`, if any.
    pub explicit_environment: Option<EnvironmentName>,
    /// The platform the task search was pinned to, if any.
    pub platform: Option<PixiPlatformName>,
}

impl Display for UnrunnableTaskError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the task '{}' is not available",
            self.task_name.fancy_display()
        )?;
        if let Some(environment) = &self.explicit_environment {
            write!(f, " in environment '{}'", environment.fancy_display())
        } else if let Some(platform) = &self.platform {
            write!(f, " for platform '{platform}'")
        } else {
            write!(f, " on this machine")
        }
    }
}

impl Diagnostic for UnrunnableTaskError {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        let first = self.environments.first()?;
        Some(Box::new(format!(
            "The task is defined in environment(s): {}.\n\nRun it there with:\n\n\tpixi run --environment {} {}",
            self.environments
                .iter()
                .map(|env| env.as_str())
                .format(", "),
            first,
            self.task_name,
        )))
    }
}

// TODO: We should make this error much better
#[derive(Debug, Error)]
pub struct AmbiguousTaskError {
    pub task_name: TaskName,
    pub environments: Vec<EnvironmentName>,
}

impl Display for AmbiguousTaskError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "the task '{}' is ambiguous", self.task_name)
    }
}

impl Diagnostic for AmbiguousTaskError {
    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        Some(Box::new(format!(
            "These environments provide the task '{task_name}': {}\n\nSpecify the '--environment' flag to run the task in a specific environment, e.g:.\n\n\tpixi run --environment {} {task_name}",
            self.environments
                .iter()
                .map(|env| env.as_str())
                .format(", "),
            self.environments
                .first()
                .expect("there should be at least two environment"),
            task_name = self.task_name
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
