use miette::Diagnostic;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
#[error("could not find the task '{task_name}'")]
pub struct MissingTaskError {
    pub task_name: String,
}
