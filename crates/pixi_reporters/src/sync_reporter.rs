use std::{cmp::Ordering, collections::HashMap, sync::Arc};

use futures::{Stream, StreamExt};
use indicatif::MultiProgress;
use parking_lot::Mutex;
use pixi_command_dispatcher::{
    BackendSourceBuildSpec, ReporterContext, SourceBuildSpec,
    reporter::{
        BackendSourceBuildId, BackendSourceBuildReporter, SourceBuildId, SourceBuildReporter,
    },
};
use pixi_progress::ProgressBarPlacement;
use rattler::{install::Transaction, package_cache::CacheReporter};
use rattler_conda_types::{PrefixRecord, RepoDataRecord};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::{
    download_verify_reporter::BuildDownloadVerifyReporter,
    main_progress_bar::{MainProgressBar, Tracker},
};

#[derive(Clone)]
pub struct SyncReporter {
    multi_progress: MultiProgress,
    combined_inner: Arc<Mutex<CombinedInstallReporterInner>>,
}

impl SyncReporter {
    pub fn new(
        multi_progress: MultiProgress,
        progress_bar_placement: ProgressBarPlacement,
    ) -> Self {
        let combined_inner = Arc::new(Mutex::new(CombinedInstallReporterInner::new(
            multi_progress.clone(),
            progress_bar_placement,
        )));
        Self {
            multi_progress,
            combined_inner,
        }
    }

    pub fn clear(&mut self) {
        let mut inner = self.combined_inner.lock();
        inner.preparing_progress_bar.clear();
        inner.install_progress_bar.clear();
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

    /// Creates a new reporter instance that can used to report the progress of
    /// fetching a single `RepoDataRecord` to the cache.
    pub fn create_cache_reporter(
        &self,
        repo_data_record: &RepoDataRecord,
    ) -> Box<dyn CacheReporter> {
        Box::new(SyncCacheReporter::new(
            repo_data_record,
            Arc::clone(&self.combined_inner),
        ))
    }
}

impl BackendSourceBuildReporter for SyncReporter {
    fn on_queued(
        &mut self,
        reason: Option<ReporterContext>,
        _env: &BackendSourceBuildSpec,
    ) -> BackendSourceBuildId {
        // Find the source build that was the reason for this build. This has queued a
        // task to the progress bar.
        let Some(ReporterContext::SourceBuild(source_build)) = reason else {
            unreachable!("SourceBuildReporter should only be called with a SourceBuild context");
        };
        BackendSourceBuildId(source_build.0)
    }

    fn on_started(
        &mut self,
        _id: BackendSourceBuildId,
        mut backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    ) {
        // Enable streaming of the logs from the backend
        let print_backend_output = tracing::event_enabled!(tracing::Level::WARN);
        // Stream the progress of the output to the screen.
        let progress_bar = self.multi_progress.clone();

        // Create a sender to buffer the output lines so we can output them later if
        // needed.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        if !print_backend_output {
            let mut inner = self.combined_inner.lock();
            inner.build_output_receiver = Some(rx);
        }

        tokio::spawn(async move {
            while let Some(line) = backend_output_stream.next().await {
                if print_backend_output {
                    // Suspend the main progress bar while we print the line.
                    progress_bar.suspend(|| eprintln!("{}", line));
                } else {
                    // Send the line to the receiver
                    if tx.send(line).is_err() {
                        // Receiver dropped, exit early
                        break;
                    }
                }
            }
        });
    }

    fn on_finished(&mut self, _id: BackendSourceBuildId, failed: bool) {
        // Take the stream that receives the output from the backend so we can drop the
        // memory.
        let build_output_receiver = {
            let mut inner = self.combined_inner.lock();
            inner.build_output_receiver.take()
        };

        // If the build failed, we want to print the output from the backend.
        let progress_bar = self.multi_progress.clone();
        if failed {
            if let Some(mut build_output_receiver) = build_output_receiver {
                tokio::spawn(async move {
                    while let Some(line) = build_output_receiver.recv().await {
                        // Suspend the main progress bar while we print the line.
                        progress_bar.suspend(|| eprintln!("{}", line));
                    }
                });
            }
        }
    }
}

impl SourceBuildReporter for SyncReporter {
    fn on_queued(
        &mut self,
        _reason: Option<ReporterContext>,
        env: &SourceBuildSpec,
    ) -> SourceBuildId {
        let mut inner = self.combined_inner.lock();
        let id = inner.preparing_progress_bar.on_build_queued(env);
        SourceBuildId(id)
    }

    fn on_started(&mut self, id: SourceBuildId) {
        // Notify the progress bar that the build has started.
        let mut inner = self.combined_inner.lock();
        inner.preparing_progress_bar.on_build_start(id.0);
    }

    fn on_finished(&mut self, id: SourceBuildId) {
        let mut inner = self.combined_inner.lock();
        inner.preparing_progress_bar.on_build_finished(id.0);
    }
}

pub struct CombinedInstallReporterInner {
    next_id: std::sync::atomic::AtomicUsize,

    operation_link_id: HashMap<(TransactionId, usize), usize>,
    cache_entry_id: HashMap<(TransactionId, usize), usize>,

    preparing_progress_bar: BuildDownloadVerifyReporter,
    install_progress_bar: MainProgressBar<PackageWithSize>,

    build_output_receiver: Option<UnboundedReceiver<String>>,
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
        let preparing_progress_bar = BuildDownloadVerifyReporter::new(
            multi_progress.clone(),
            progress_bar_placement.clone(),
            "preparing packages".to_owned(),
        );
        let link_progress_bar = MainProgressBar::new(
            multi_progress.clone(),
            ProgressBarPlacement::After(preparing_progress_bar.progress_bar()),
            "installing".to_owned(),
        );

        Self {
            next_id: std::sync::atomic::AtomicUsize::new(0),
            preparing_progress_bar,
            install_progress_bar: link_progress_bar,
            operation_link_id: HashMap::new(),
            cache_entry_id: HashMap::new(),
            build_output_receiver: None,
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
                    self.install_progress_bar.queued(PackageWithSize {
                        name: record.package_record.name.as_normalized().to_string(),
                        size: record.package_record.size.unwrap_or(1),
                    }),
                );
            }
            if let Some(record) = operation.record_to_install() {
                self.cache_entry_id.insert(
                    (id, operation_id),
                    self.preparing_progress_bar.on_entry_start(record),
                );
            }
        }
    }

    /// Called to start progress on populating the cache for a single
    /// `RepoDataRecord`.
    fn start_populate_single_cache_entry(
        &mut self,
        record: &RepoDataRecord,
    ) -> (TransactionId, usize) {
        let transaction_id = TransactionId(
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        );
        let operation_id = 0;
        let cache_entry_id = self.preparing_progress_bar.on_entry_start(record);
        self.cache_entry_id
            .insert((transaction_id, operation_id), cache_entry_id);
        (transaction_id, cache_entry_id)
    }

    fn on_transaction_operation_start(&mut self, _id: TransactionId, _operation: usize) {}

    fn on_populate_cache_start(
        &mut self,
        id: TransactionId,
        operation: usize,
        _record: &RepoDataRecord,
    ) -> usize {
        *self
            .cache_entry_id
            .get(&(id, operation))
            .expect("missing operation link")
    }

    fn on_validate_start(&mut self, _id: TransactionId, cache_entry: usize) -> usize {
        self.preparing_progress_bar.on_validation_start(cache_entry);
        cache_entry
    }

    fn on_validate_complete(&mut self, _id: TransactionId, validation_id: usize) {
        self.preparing_progress_bar
            .on_validation_complete(validation_id);
    }

    fn on_download_start(&mut self, _id: TransactionId, cache_entry: usize) -> usize {
        self.preparing_progress_bar.on_download_start(cache_entry);
        cache_entry
    }

    fn on_download_progress(
        &mut self,
        _id: TransactionId,
        cache_entry: usize,
        progress: u64,
        total: Option<u64>,
    ) {
        self.preparing_progress_bar
            .on_download_progress(cache_entry, progress, total);
    }

    fn on_download_completed(&mut self, _id: TransactionId, cache_entry: usize) {
        self.preparing_progress_bar
            .on_download_complete(cache_entry);
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
            self.install_progress_bar.start(link_id)
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
            self.install_progress_bar.start(link_id)
        };
        operation
    }

    fn on_link_complete(&mut self, _id: TransactionId, _index: usize) {}

    fn on_transaction_operation_complete(&mut self, id: TransactionId, operation: usize) {
        if let Some(link_id) = self.operation_link_id.remove(&(id, operation)) {
            self.install_progress_bar.finish(link_id);
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

struct SyncCacheReporter {
    combined_inner: Arc<Mutex<CombinedInstallReporterInner>>,
    transaction_id: TransactionId,
    cache_entry_id: usize,
}

impl SyncCacheReporter {
    pub fn new(
        record: &RepoDataRecord,
        combined_inner: Arc<Mutex<CombinedInstallReporterInner>>,
    ) -> Self {
        let (transaction_id, cache_entry_id) = {
            let mut inner = combined_inner.lock();
            inner.start_populate_single_cache_entry(record)
        };
        Self {
            combined_inner,
            transaction_id,
            cache_entry_id,
        }
    }
}

impl CacheReporter for SyncCacheReporter {
    fn on_validate_start(&self) -> usize {
        let mut inner = self.combined_inner.lock();
        inner.on_validate_start(self.transaction_id, self.cache_entry_id)
    }

    fn on_validate_complete(&self, index: usize) {
        let mut inner = self.combined_inner.lock();
        inner.on_validate_complete(self.transaction_id, index);
    }

    fn on_download_start(&self) -> usize {
        let mut inner = self.combined_inner.lock();
        inner.on_download_start(self.transaction_id, self.cache_entry_id)
    }

    fn on_download_progress(&self, index: usize, progress: u64, total: Option<u64>) {
        let mut inner = self.combined_inner.lock();
        inner.on_download_progress(self.transaction_id, index, progress, total);
    }

    fn on_download_completed(&self, index: usize) {
        let mut inner = self.combined_inner.lock();
        inner.on_download_completed(self.transaction_id, index);
    }
}
