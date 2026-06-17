//! Reporter traits for the PyPI solve and install operations.
//!
//! These mirror the per-key reporter mechanism used by the conda solve and
//! install paths: implementations are registered as `Arc<dyn ...>` objects
//! in the engine's [`DataStore`] and receive queued/started/finished
//! lifecycle callbacks. The `create_uv_reporter` hooks let the
//! implementation hand out a [`UvReporter`] that uv's resolver, preparer,
//! and installer report their detailed progress on; returning `None`
//! disables the detailed progress for that operation.

use std::sync::Arc;

use pixi_compute_engine::DataStore;
use pixi_compute_reporters::OperationId;
use pixi_uv_reporter::{UvReporter, UvReporterOptions};

/// Reports on the lifecycle of a PyPI resolve.
pub trait SolvePypiReporter: Send + Sync {
    /// Called when a PyPI resolve is queued. `name` identifies the
    /// environment being resolved.
    fn on_queued(&self, name: &str, platform: &str) -> OperationId;

    /// Called when the resolve starts.
    fn on_started(&self, id: OperationId);

    /// Called when the resolve finished (successfully or not).
    fn on_finished(&self, id: OperationId);

    /// Returns the reporter on which uv reports resolution progress.
    fn create_uv_reporter(
        &self,
        id: OperationId,
        options: UvReporterOptions,
    ) -> Option<Arc<UvReporter>>;
}

/// Reports on the lifecycle of a PyPI install.
pub trait InstallPypiReporter: Send + Sync {
    /// Called when a PyPI install is queued. `name` identifies the
    /// environment being installed.
    fn on_queued(&self, name: &str) -> OperationId;

    /// Called when the install starts.
    fn on_started(&self, id: OperationId);

    /// Called when the install finished (successfully or not).
    fn on_finished(&self, id: OperationId);

    /// Returns the reporter on which uv reports preparation (download and
    /// build) and installation progress.
    fn create_uv_reporter(
        &self,
        id: OperationId,
        options: UvReporterOptions,
    ) -> Option<Arc<UvReporter>>;
}

/// Access the per-key PyPI solve reporter.
pub trait HasSolvePypiReporter {
    fn solve_pypi_reporter(&self) -> Option<&Arc<dyn SolvePypiReporter>>;
}

impl HasSolvePypiReporter for DataStore {
    fn solve_pypi_reporter(&self) -> Option<&Arc<dyn SolvePypiReporter>> {
        self.try_get::<Arc<dyn SolvePypiReporter>>()
    }
}

/// Access the per-key PyPI install reporter.
pub trait HasInstallPypiReporter {
    fn install_pypi_reporter(&self) -> Option<&Arc<dyn InstallPypiReporter>>;
}

impl HasInstallPypiReporter for DataStore {
    fn install_pypi_reporter(&self) -> Option<&Arc<dyn InstallPypiReporter>> {
        self.try_get::<Arc<dyn InstallPypiReporter>>()
    }
}
