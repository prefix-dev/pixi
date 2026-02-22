use rattler_repodata_gateway::Gateway;

pub trait Repodata {
    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> miette::Result<&Gateway>;
}
