use crate::project::Workspace;
use crate::repodata::Repodata;
use rattler_repodata_gateway::Gateway;

impl Repodata for Workspace {
    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> &Gateway {
        self.repodata_gateway
            .get_or_init(|| self.config().gateway(self.authenticated_client().clone()))
    }
}
