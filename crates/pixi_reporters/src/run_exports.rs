use rattler::package_cache::CacheReporter;
use rattler_conda_types::RepoDataRecord;
use rattler_repodata_gateway::DownloadReporter;

use crate::{repodata_reporter::RepodataReporter, sync_reporter::SyncReporter};

pub struct RunExportsReporter {
    /// The reporter for repodata fetching (used for reporting progress on
    /// run_exports.json fetching).
    repodata_reporter: RepodataReporter,

    /// The reporter to use when downloading a package.
    sync_reporter: SyncReporter,
}

impl RunExportsReporter {
    pub fn new(repodata_reporter: RepodataReporter, sync_reporter: SyncReporter) -> Self {
        Self {
            repodata_reporter,
            sync_reporter,
        }
    }
}

impl rattler_repodata_gateway::RunExportsReporter for RunExportsReporter {
    fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
        Some(&self.repodata_reporter)
    }

    fn create_package_download_reporter(
        &self,
        repo_data_record: &RepoDataRecord,
    ) -> Option<Box<dyn CacheReporter>> {
        Some(self.sync_reporter.create_cache_reporter(repo_data_record))
    }
}
