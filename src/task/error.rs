use crate::project::manifest::EnvironmentName;
use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
#[error("could not find the task '{task_name}'")]
pub struct MissingTaskError {
    pub task_name: String,
}

// TODO: We should make this error much better
#[derive(Debug, Error, Diagnostic)]
#[error("'{task_name}' is ambiguous")]
pub struct AmbiguousTaskError {
    pub task_name: String,
    pub environments: Vec<EnvironmentName>,
}
