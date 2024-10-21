use std::sync::Arc;

use pixi_build_frontend::{CondaBuildReporter, CondaMetadataReporter};

pub trait BuildMetadataReporter: CondaMetadataReporter {
    /// Reporters that the metadata has been cached.
    fn on_metadata_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter>;
}

/// Noop implementation of the BuildMetadataReporter trait.
struct NoopBuildMetadataReporter;
impl CondaMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_metadata_end(&self, _operation: usize) {}
}
impl BuildMetadataReporter for NoopBuildMetadataReporter {
    fn on_metadata_cached(&self, _build_id: usize) {}

    fn as_conda_metadata_reporter(self: Arc<Self>) -> Arc<dyn CondaMetadataReporter> {
        self
    }
}

pub trait BuildReporter: CondaBuildReporter {
    /// Reports that the build has been cached.
    fn on_build_cached(&self, build_id: usize);

    /// Cast upwards
    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter>;
}

/// Noop implementation of the BuildReporter trait.
struct NoopBuildReporter;
impl CondaBuildReporter for NoopBuildReporter {
    fn on_build_start(&self, _build_id: usize) -> usize {
        0
    }

    fn on_build_end(&self, _operation: usize) {}

    fn on_build_output(&self, _operation: usize, _line: String) {}
}
impl BuildReporter for NoopBuildReporter {
    fn on_build_cached(&self, _build_id: usize) {}

    fn as_conda_build_reporter(self: Arc<Self>) -> Arc<dyn CondaBuildReporter> {
        self
    }
}
