use crate::project::Project;
use crate::repodata::Repodata;
use rattler_repodata_gateway::Gateway;

impl Repodata for Project {
    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> miette::Result<&Gateway> {
        self.repodata_gateway
            .get_or_try_init(|| Ok(self.config().gateway(self.authenticated_client()?.clone())))
    }
}
