use rattler_repodata_gateway::Gateway;

use crate::{repodata::Repodata, workspace::Workspace};

impl Repodata for Workspace {
    /// Returns the [`Gateway`] used by this project.
    fn repodata_gateway(&self) -> miette::Result<&Gateway> {
        self.repodata_gateway.get_or_try_init(|| {
            let client = self.authenticated_client()?.clone();
            let concurrent_downloads = self.concurrent_downloads_semaphore();
            Ok(self
                .config()
                .gateway()
                .with_client(client)
                .with_max_concurrent_requests(concurrent_downloads)
                .finish())
        })
    }
}
