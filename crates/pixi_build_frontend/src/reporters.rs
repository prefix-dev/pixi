use std::sync::Arc;

/// Reporter trait for reporting the progress of metadata operations.
pub trait CondaMetadataReporter: Send + Sync {
    /// Reports the start of the get_conda_metadata operation.
    fn on_metadata_start(&self, build_id: usize) -> usize;
    /// Reports the end of the get_conda_metadata operation.
    fn on_metadata_end(&self, operation: usize);
}

/// A no-op implementation of the CondaMetadataReporter trait.
#[derive(Clone)]
pub struct NoopCondaMetadataReporter;
impl CondaMetadataReporter for NoopCondaMetadataReporter {
    fn on_metadata_start(&self, _build_id: usize) -> usize {
        0
    }
    fn on_metadata_end(&self, _operation: usize) {}
}

impl NoopCondaMetadataReporter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {})
    }
}

/// Reporter trait for reporting the progress of build operations.
pub trait CondaBuildReporter: Send + Sync {
    /// Reports the start of the build_conda operation.
    fn on_build_start(&self, build_id: usize) -> usize;
    /// Reports the end of the build_conda operation.
    fn on_build_end(&self, operation: usize);
}

/// A no-op implementation of the CondaBuildReporter trait.
#[derive(Clone)]
pub struct NoopCondaBuildReporter;
impl CondaBuildReporter for NoopCondaBuildReporter {
    fn on_build_start(&self, _build_id: usize) -> usize {
        0
    }
    fn on_build_end(&self, _operation: usize) {}
}

impl NoopCondaBuildReporter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {})
    }
}
