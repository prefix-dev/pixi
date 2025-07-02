use rattler::install::Transaction;
use rattler_conda_types::{PrefixRecord, RepoDataRecord};

pub struct WrappingInstallReporter(pub Box<dyn rattler::install::Reporter>);

impl rattler::install::Reporter for WrappingInstallReporter {
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>) {
        self.0.on_transaction_start(transaction)
    }

    fn on_transaction_operation_start(&self, operation: usize) {
        self.0.on_transaction_operation_start(operation)
    }

    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        self.0.on_populate_cache_start(operation, record)
    }

    fn on_validate_start(&self, cache_entry: usize) -> usize {
        self.0.on_validate_start(cache_entry)
    }

    fn on_validate_complete(&self, validate_idx: usize) {
        self.0.on_validate_complete(validate_idx)
    }

    fn on_download_start(&self, cache_entry: usize) -> usize {
        self.0.on_download_start(cache_entry)
    }

    fn on_download_progress(&self, download_idx: usize, progress: u64, total: Option<u64>) {
        self.0.on_download_progress(download_idx, progress, total)
    }

    fn on_download_completed(&self, download_idx: usize) {
        self.0.on_download_completed(download_idx)
    }

    fn on_populate_cache_complete(&self, cache_entry: usize) {
        self.0.on_populate_cache_complete(cache_entry)
    }

    fn on_unlink_start(&self, operation: usize, record: &PrefixRecord) -> usize {
        self.0.on_unlink_start(operation, record)
    }

    fn on_unlink_complete(&self, index: usize) {
        self.0.on_unlink_complete(index)
    }

    fn on_link_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        self.0.on_link_start(operation, record)
    }

    fn on_link_complete(&self, index: usize) {
        self.0.on_link_complete(index)
    }

    fn on_transaction_operation_complete(&self, operation: usize) {
        self.0.on_transaction_operation_complete(operation)
    }

    fn on_transaction_complete(&self) {
        self.0.on_transaction_complete()
    }
}
