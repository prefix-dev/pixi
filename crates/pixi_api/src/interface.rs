use miette::Result;
use std::future::Future;

pub trait Interface {
    fn is_cli(&self) -> impl Future<Output = bool> + Send;
    fn confirm(&self, msg: &str) -> impl Future<Output = Result<bool>> + Send;
    fn select(
        &self,
        msg: &str,
        options: &[String],
    ) -> impl Future<Output = Result<Option<usize>>> + Send;
    fn info(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn success(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn warning(&self, msg: &str) -> impl Future<Output = ()> + Send;
    fn error(&self, msg: &str) -> impl Future<Output = ()> + Send;
}
