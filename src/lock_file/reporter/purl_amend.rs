use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use rattler_conda_types::RepoDataRecord;

use crate::lock_file::reporter::SolveProgressBar;

pub(crate) struct PurlAmendReporter {
    pb: Arc<SolveProgressBar>,
    style_set: AtomicBool,
}

impl PurlAmendReporter {
    pub(super) fn new(pb: Arc<SolveProgressBar>) -> Self {
        Self {
            pb,
            style_set: AtomicBool::new(false),
        }
    }
}

impl pypi_mapping::Reporter for PurlAmendReporter {
    fn download_started(&self, _package: &RepoDataRecord, total: usize) {
        if !self.style_set.swap(true, Ordering::Relaxed) {
            self.pb.set_update_style(total);
        }
    }

    fn download_finished(&self, _package: &RepoDataRecord, _total: usize) {
        self.pb.inc(1);
    }

    fn download_failed(&self, package: &RepoDataRecord, total: usize) {
        self.download_finished(package, total);
    }
}
