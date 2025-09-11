use miette::Result;
use std::future::Future;

pub trait Interface {
    fn is_cli(&self) -> impl Future<Output = bool> + Send;
    fn confirm(&self, msg: &str) -> impl Future<Output = Result<bool>> + Send;
    fn message(&self, msg: &str) -> impl Future<Output = Result<()>> + Send;
    fn success(&self, msg: &str) -> impl Future<Output = Result<()>> + Send;
    fn warning(&self, msg: &str) -> impl Future<Output = Result<()>> + Send;
    fn error(&self, msg: &str) -> impl Future<Output = Result<()>> + Send;
}
