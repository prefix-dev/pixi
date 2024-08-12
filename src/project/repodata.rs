use crate::project::Project;

use rattler_repodata_gateway::Gateway;

impl Project {
    /// Returns the [`Gateway`] used by this project.
    pub fn repodata_gateway(&self) -> &Gateway {
        self.repodata_gateway
            .get_or_init(|| self.config.gateway(self.authenticated_client().clone()))
    }
}
