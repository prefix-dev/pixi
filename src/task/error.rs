use crate::project::manifest::EnvironmentName;
use itertools::Itertools;
use miette::Diagnostic;
use std::fmt::{Display, Formatter};
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
#[error("could not find the task '{task_name}'")]
pub struct MissingTaskError {
    pub task_name: String,
}

// TODO: We should make this error much better
#[derive(Debug, Error)]
pub struct AmbiguousTaskError {
    pub task_name: String,
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
            self.environments.iter().map(|env| env.as_str()).format(", "),
            env!("CARGO_PKG_NAME"),
            self.environments.first().expect("there should be at least two environment"),
            task_name=&self.task_name
        )))
    }
}
