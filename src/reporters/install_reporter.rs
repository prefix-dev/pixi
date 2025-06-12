use std::{cmp::Ordering, collections::HashMap, sync::Arc};

use indicatif::MultiProgress;
use parking_lot::Mutex;
use pixi_command_dispatcher::{
    InstallPixiEnvironmentSpec, ReporterContext, reporter::PixiInstallId,
};
use pixi_progress::ProgressBarPlacement;
use rattler::install::Transaction;
use rattler_conda_types::{PrefixRecord, RepoDataRecord};

use crate::reporters::{
    download_verify_reporter::DownloadVerifyReporter,
    main_progress_bar::{MainProgressBar, Tracker},
};

pub struct SyncReporter {
    sync_pb: MainProgressBar<String>,
    combined_inner: Arc<Mutex<CombinedInstallReporterInner>>,
}

impl SyncReporter {
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
    ) -> Self {
        let sync_pb = MainProgressBar::new(
            multi_progress.clone(),
            progress_bar_placement,
            "syncing".to_owned(),
        );
        let combined_inner = Arc::new(Mutex::new(CombinedInstallReporterInner::new(
            multi_progress.clone(),
            ProgressBarPlacement::Before(sync_pb.progress_bar()),
        )));
        Self {
            sync_pb,
            combined_inner,
        }
    }

    pub fn clear(&mut self) {
        self.sync_pb.clear();
        let mut inner = self.combined_inner.lock();
        inner.preparing_progress_bar.clear();
        inner.link_progress_bar.clear();
    }

    /// Creates a new InstallReporter that shares this SyncReporter instance
    pub fn create_reporter(&self) -> InstallReporter {
        let id = self
            .combined_inner
            .lock()
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        InstallReporter {
            id: TransactionId::new(id),
            combined: Arc::clone(&self.combined_inner),
        }
    }
}

impl pixi_command_dispatcher::PixiInstallReporter for SyncReporter {
    fn on_queued(
        &mut self,
        _reason: Option<ReporterContext>,
        env: &InstallPixiEnvironmentSpec,
    ) -> PixiInstallId {
        let id = self.sync_pb.queued(env.name.clone());
        PixiInstallId(id)
    }

    fn on_start(&mut self, solve_id: PixiInstallId) {
        self.sync_pb.start(solve_id.0);
    }

    fn on_finished(&mut self, solve_id: PixiInstallId) {
        self.sync_pb.finish(solve_id.0);
    }
}

pub struct CombinedInstallReporterInner {
    next_id: std::sync::atomic::AtomicUsize,

    operation_link_id: HashMap<(TransactionId, usize), usize>,

    preparing_progress_bar: DownloadVerifyReporter,
    link_progress_bar: MainProgressBar<PackageWithSize>,
}

#[derive(PartialEq, Eq)]
pub struct PackageWithSize {
    pub name: String,
    pub size: u64,
}

impl Tracker for PackageWithSize {
    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn size(&self) -> u64 {
        self.size
    }
}

impl Ord for PackageWithSize {
    fn cmp(&self, other: &Self) -> Ordering {
        self.size.cmp(&other.size).reverse()
    }
}

impl PartialOrd for PackageWithSize {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl CombinedInstallReporterInner {
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
    ) -> Self {
        let preparing_progress_bar = DownloadVerifyReporter::new(
            multi_progress.clone(),
            progress_bar_placement.clone(),
            "preparing packages".to_owned(),
        );
        let link_progress_bar = MainProgressBar::new(
            multi_progress.clone(),
            ProgressBarPlacement::After(preparing_progress_bar.progress_bar()),
            "linking packages".to_owned(),
        );

        Self {
            next_id: std::sync::atomic::AtomicUsize::new(0),
            preparing_progress_bar,
            link_progress_bar,
            operation_link_id: HashMap::new(),
        }
    }

    fn on_transaction_start(
        &mut self,
        id: TransactionId,
        transaction: &Transaction<PrefixRecord, RepoDataRecord>,
    ) {
        for (operation_id, operation) in transaction.operations.iter().enumerate() {
            if let Some(record) = operation
                .record_to_install()
                .or_else(|| operation.record_to_remove().map(|r| &r.repodata_record))
            {
                self.operation_link_id.insert(
                    (id, operation_id),
                    self.link_progress_bar.queued(PackageWithSize {
                        name: record.package_record.name.as_normalized().to_string(),
                        size: record.package_record.size.unwrap_or(1),
                    }),
                );
            }
            if let Some(record) = operation.record_to_install() {
                self.preparing_progress_bar.on_entry_start(record);
            }
        }
    }

    fn on_transaction_operation_start(&mut self, _id: TransactionId, _operation: usize) {}

    fn on_populate_cache_start(
        &mut self,
        id: TransactionId,
        operation: usize,
        _record: &RepoDataRecord,
    ) -> usize {
        *self
            .operation_link_id
            .get(&(id, operation))
            .expect("missing operation link")
    }

    fn on_validate_start(&mut self, _id: TransactionId, cache_entry: usize) -> usize {
        self.preparing_progress_bar.on_validation_start(cache_entry);
        cache_entry
    }

    fn on_validate_complete(&mut self, _id: TransactionId, validate_idx: usize) {
        self.preparing_progress_bar
            .on_validation_complete(validate_idx);
    }

    fn on_download_start(&mut self, _id: TransactionId, cache_entry: usize) -> usize {
        self.preparing_progress_bar.on_download_start(cache_entry);
        cache_entry
    }

    fn on_download_progress(
        &mut self,
        _id: TransactionId,
        download_idx: usize,
        progress: u64,
        total: Option<u64>,
    ) {
        self.preparing_progress_bar
            .on_download_progress(download_idx, progress, total);
    }

    fn on_download_completed(&mut self, _id: TransactionId, download_idx: usize) {
        self.preparing_progress_bar
            .on_download_complete(download_idx);
    }

    fn on_populate_cache_complete(&mut self, _id: TransactionId, cache_entry: usize) {
        self.preparing_progress_bar.on_entry_finished(cache_entry);
    }

    fn on_unlink_start(
        &mut self,
        id: TransactionId,
        operation: usize,
        _record: &PrefixRecord,
    ) -> usize {
        if let Some(&link_id) = self.operation_link_id.get(&(id, operation)) {
            self.link_progress_bar.start(link_id)
        };
        operation
    }

    fn on_unlink_complete(&mut self, _id: TransactionId, _index: usize) {}

    fn on_link_start(
        &mut self,
        id: TransactionId,
        operation: usize,
        _record: &RepoDataRecord,
    ) -> usize {
        if let Some(&link_id) = self.operation_link_id.get(&(id, operation)) {
            self.link_progress_bar.start(link_id)
        };
        operation
    }

    fn on_link_complete(&mut self, _id: TransactionId, _index: usize) {}

    fn on_transaction_operation_complete(&mut self, id: TransactionId, operation: usize) {
        if let Some(link_id) = self.operation_link_id.remove(&(id, operation)) {
            self.link_progress_bar.finish(link_id);
        }
    }

    fn on_transaction_complete(&mut self, _id: TransactionId) {}
}

pub struct InstallReporter {
    id: TransactionId,
    combined: Arc<Mutex<CombinedInstallReporterInner>>,
}

impl rattler::install::Reporter for InstallReporter {
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>) {
        self.combined
            .lock()
            .on_transaction_start(self.id, transaction)
    }

    fn on_transaction_operation_start(&self, operation: usize) {
        self.combined
            .lock()
            .on_transaction_operation_start(self.id, operation)
    }

    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        self.combined
            .lock()
            .on_populate_cache_start(self.id, operation, record)
    }

    fn on_validate_start(&self, cache_entry: usize) -> usize {
        self.combined.lock().on_validate_start(self.id, cache_entry)
    }

    fn on_validate_complete(&self, validate_idx: usize) {
        self.combined
            .lock()
            .on_validate_complete(self.id, validate_idx)
    }

    fn on_download_start(&self, cache_entry: usize) -> usize {
        self.combined.lock().on_download_start(self.id, cache_entry)
    }

    fn on_download_progress(&self, download_idx: usize, progress: u64, total: Option<u64>) {
        self.combined
            .lock()
            .on_download_progress(self.id, download_idx, progress, total)
    }

    fn on_download_completed(&self, download_idx: usize) {
        self.combined
            .lock()
            .on_download_completed(self.id, download_idx)
    }

    fn on_populate_cache_complete(&self, cache_entry: usize) {
        self.combined
            .lock()
            .on_populate_cache_complete(self.id, cache_entry)
    }

    fn on_unlink_start(&self, operation: usize, record: &PrefixRecord) -> usize {
        self.combined
            .lock()
            .on_unlink_start(self.id, operation, record)
    }

    fn on_unlink_complete(&self, index: usize) {
        self.combined.lock().on_unlink_complete(self.id, index)
    }

    fn on_link_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        self.combined
            .lock()
            .on_link_start(self.id, operation, record)
    }

    fn on_link_complete(&self, index: usize) {
        self.combined.lock().on_link_complete(self.id, index)
    }

    fn on_transaction_operation_complete(&self, operation: usize) {
        self.combined
            .lock()
            .on_transaction_operation_complete(self.id, operation)
    }

    fn on_transaction_complete(&self) {
        self.combined.lock().on_transaction_complete(self.id)
    }
}

/// A type-safe identifier for transactions to avoid confusion with other IDs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransactionId(pub usize);

impl TransactionId {
    pub fn new(id: usize) -> Self {
        TransactionId(id)
    }
}
