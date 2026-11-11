//! `ctx.backend_source_build` extension trait. Runs a single backend
//! source build with concurrency limiting and progress reporting.
//!
//! The shared build body lives in [`BackendSourceBuildSpec::build`]; this
//! ext only handles the semaphore, reporter lifecycle, log channel, and
//! reporter-context scoping.

use pixi_compute_engine::{ComputeCtx, DataStore};

use crate::CommandDispatcherError;
use crate::backend_source_build::{
    BackendBuiltSource, BackendSourceBuildError, BackendSourceBuildSpec,
};
use crate::compute_data::{HasBackendSourceBuildReporter, HasBackendSourceBuildSemaphore};
use crate::injected_config::ChannelConfigKey;

/// Extension trait on [`ComputeCtx`] that runs a backend source build with
/// concurrency limiting and progress reporting.
pub trait BackendSourceBuildExt {
    /// Reports progress via `Arc<dyn BackendSourceBuildReporter>` set on the engine `DataStore`, if any.
    fn backend_source_build(
        &mut self,
        spec: BackendSourceBuildSpec,
    ) -> impl Future<
        Output = Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>>,
    > + Send;
}

impl BackendSourceBuildExt for ComputeCtx {
    async fn backend_source_build(
        &mut self,
        spec: BackendSourceBuildSpec,
    ) -> Result<BackendBuiltSource, CommandDispatcherError<BackendSourceBuildError>> {
        let channel_config = self.compute(&ChannelConfigKey).await;
        let data: &DataStore = self.global_data();
        let semaphore = data.backend_source_build_semaphore().cloned();
        let reporter_arc = data.backend_source_build_reporter().cloned();

        let reporter_id = reporter_arc.as_deref().map(|r| r.on_queued(&spec));

        let _permit = match semaphore.as_ref() {
            Some(s) => Some(
                s.acquire()
                    .await
                    .expect("backend-source-build semaphore is never closed"),
            ),
            None => None,
        };

        // on_started carries the log-stream receiver so the reporter can
        // tee backend output into the UI as it arrives.
        let (log_sink, log_rx) = futures::channel::mpsc::unbounded::<String>();
        if let (Some(r), Some(id)) = (reporter_arc.as_deref(), reporter_id) {
            r.on_started(id, Box::new(log_rx));
        }

        // Scope nested work under this build's id.
        let work = spec.build(channel_config, log_sink);
        let result = match reporter_id {
            Some(id) => id.scope_active(work).await,
            None => work.await,
        };

        if let (Some(r), Some(id)) = (reporter_arc.as_deref(), reporter_id) {
            r.on_finished(id, result.is_err());
        }

        result
    }
}
