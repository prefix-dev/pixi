pub trait Interface {
    fn is_cli(&self) -> bool;
    fn confirm(&self, msg: &str) -> miette::Result<bool>;
    fn message(&self, msg: &str);
    fn success(&self, msg: &str);
    fn warning(&self, msg: &str);
    fn error(&self, msg: &str);
}
