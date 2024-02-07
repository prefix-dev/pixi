use crate::project::manifest::EnvironmentName;
use crate::task::TaskName;
use crate::Project;
use itertools::Itertools;
use miette::{Diagnostic, LabeledSpan};
use rattler_conda_types::Platform;
use std::error::Error;
use std::fmt::{Display, Formatter};
use thiserror::Error;

/// An error that occurs when data is requested for a platform that is not supported.
/// TODO: Make this error better by also explaining to the user why a certain platform was not
///  supported and with suggestions as how to fix it.
#[derive(Debug, Clone)]
pub struct UnsupportedPlatformError {
    /// Platforms supported by the environment
    pub environments_platforms: Vec<Platform>,

    /// The environment that the platform is not supported for.
    pub environment: EnvironmentName,

    /// The platform that was requested
    pub platform: Platform,
}

impl Error for UnsupportedPlatformError {}

impl Display for UnsupportedPlatformError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.environment {
            EnvironmentName::Default => {
                write!(f, "the project does not support '{}'", self.platform)
            }
            EnvironmentName::Named(name) => write!(
                f,
                "the environment '{}' does not support '{}'",
                name, self.platform
            ),
        }
    }
}

impl Diagnostic for UnsupportedPlatformError {
    fn code(&self) -> Option<Box<dyn Display + '_>> {
        Some(Box::new("unsupported-platform".to_string()))
    }

    fn help(&self) -> Option<Box<dyn Display + '_>> {
        Some(Box::new(format!(
            "supported platforms are {}",
            self.environments_platforms.iter().format(", ")
        )))
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        None
    }
}

/// An error that occurs when a task is requested which could not be found.
/// TODO: Make this error better.
///     - Include names that might have been meant instead
///     - If the tasks is only available for a certain platform, explain that.
#[derive(Debug, Clone, Diagnostic, Error)]
#[error("the task '{0}' could not be found", task_name.fancy_display())]
pub struct UnknownTask<'p> {
    /// The project that the platform is not supported for.
    pub project: &'p Project,

    /// The environment that the platform is not supported for.
    pub environment: EnvironmentName,

    /// The platform that was requested (if any)
    pub platform: Option<Platform>,

    /// The name of the task
    pub task_name: TaskName,
}
