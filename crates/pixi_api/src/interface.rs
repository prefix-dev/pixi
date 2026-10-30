use std::future::Future;

use miette::Diagnostic;
use thiserror::Error;

pub trait Interface {
    fn is_cli(&self) -> impl Future<Output = bool> + Send;
    fn confirm(&self, msg: &str) -> impl Future<Output = Result<bool, InterfaceError>> + Send;
    fn select(
        &self,
        msg: &str,
        options: &[String],
    ) -> impl Future<Output = Result<Option<usize>, InterfaceError>> + Send;
    fn info(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn success(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn warning(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn error(&self, msg: &str) -> impl Future<Output = ()> + Send;
}

/// An error from an [`Interface`] front-end while asking the user something.
///
/// Front-ends wrap whatever their prompt failed with, e.g.
/// `.map_err(InterfaceError::new)`.
#[derive(Debug, Error, Diagnostic)]
#[error("failed to get a response from the user")]
pub struct InterfaceError(#[source] Box<dyn std::error::Error + Send + Sync>);

impl InterfaceError {
    pub fn new(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self(error.into())
    }
}
