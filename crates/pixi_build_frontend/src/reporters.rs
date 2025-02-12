use std::sync::Arc;

/// Reporter trait for reporting the progress of metadata operations.
pub trait CondaMetadataReporter: Send + Sync {
    /// Reports the start of the conda_get_metadata operation.
    /// Returns a unique identifier for the operation.
    fn on_metadata_start(&self, build_id: usize) -> usize;

    /// Reports the end of the conda_get_metadata operation.
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
    /// Returns a unique identifier for the operation.
    fn on_build_start(&self, build_id: usize) -> usize;

    /// Reports the end of the build_conda operation.
    fn on_build_end(&self, operation: usize);

    /// Reports output from the build process.
    fn on_build_output(&self, operation: usize, line: String);
}

/// A no-op implementation of the CondaBuildReporter trait.
#[derive(Clone)]
pub struct NoopCondaBuildReporter;
impl CondaBuildReporter for NoopCondaBuildReporter {
    fn on_build_start(&self, _build_id: usize) -> usize {
        0
    }
    fn on_build_end(&self, _operation: usize) {}

    fn on_build_output(&self, _operation: usize, _line: String) {
        todo!()
    }
}

impl NoopCondaBuildReporter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {})
    }
}
