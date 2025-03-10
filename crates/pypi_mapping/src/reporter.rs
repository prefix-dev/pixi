use rattler_conda_types::RepoDataRecord;

pub trait Reporter: Send + Sync {
    fn download_started(&self, package: &RepoDataRecord, total: usize);
    fn download_finished(&self, package: &RepoDataRecord, total: usize);
    fn download_failed(&self, package: &RepoDataRecord, total: usize);
}
