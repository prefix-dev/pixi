use miette::Result;
use std::future::Future;

pub trait Interface {
    fn is_cli(&self) -> impl Future<Output = bool> + Send;
    fn confirm(&self, msg: &str) -> impl Future<Output = Result<bool>> + Send;
    fn info(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn success(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn warning(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn error(&self, msg: &str) -> impl Future<Output = ()> + Send;
}
