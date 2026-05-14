use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::{Stream, StreamExt};
use pixi_build_discovery::JsonRpcBackendSpec;
use pixi_command_dispatcher::{
    BackendSourceBuildSpec, BuildBackendMetadataInner, CondaSolveReporter, GitCheckoutReporter,
    InstallPixiEnvironmentSpec, PixiInstallReporter, PixiSolveEnvironmentSpec, PixiSolveReporter,
    SolveCondaEnvironmentSpec, SourceMetadataReporterSpec, SourceRecordReporterSpec,
    UrlCheckoutReporter,
    reporter::{
        BackendSourceBuildReporter, BuildBackendMetadataReporter, InstantiateBackendReporter,
        SourceMetadataReporter, SourceRecordReporter,
    },
};
use pixi_compute_reporters::{OperationId, OperationRegistry};
use pixi_git::resolver::RepositoryReference;
use serde::Serialize;
use url::Url;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    CondaSolveQueued {
        id: OperationId,
        #[serde(flatten)]
        spec: Box<SolveCondaEnvironmentSpec>,
    },
    CondaSolveStarted {
        id: OperationId,
    },
    CondaSolveFinished {
        id: OperationId,
    },

    PixiSolveQueued {
        id: OperationId,
        #[serde(flatten)]
        spec: PixiSolveEnvironmentSpec,
    },
    PixiSolveStarted {
        id: OperationId,
    },
    PixiSolveFinished {
        id: OperationId,
    },

    PixiInstallQueued {
        id: OperationId,
        #[serde(flatten)]
        spec: InstallPixiEnvironmentSpec,
    },
    PixiInstallStarted {
        id: OperationId,
    },
    PixiInstallFinished {
        id: OperationId,
    },

    GitCheckoutQueued {
        id: OperationId,
        #[serde(flatten)]
        reference: RepositoryReference,
    },
    GitCheckoutStarted {
        id: OperationId,
    },
    GitCheckoutFinished {
        id: OperationId,
    },

    UrlCheckoutQueued {
        id: OperationId,
        url: Url,
    },
    UrlCheckoutStarted {
        id: OperationId,
    },
    UrlCheckoutFinished {
        id: OperationId,
    },

    BuildBackendMetadataQueued {
        id: OperationId,
        #[serde(flatten)]
        spec: BuildBackendMetadataInner,
    },
    BuildBackendMetadataStarted {
        id: OperationId,
    },
    BuildBackendMetadataFinished {
        id: OperationId,
    },

    SourceMetadataQueued {
        id: OperationId,
        #[serde(flatten)]
        spec: SourceMetadataReporterSpec,
    },
    SourceMetadataStarted {
        id: OperationId,
    },
    SourceMetadataFinished {
        id: OperationId,
    },

    SourceRecordQueued {
        id: OperationId,
        #[serde(flatten)]
        spec: SourceRecordReporterSpec,
    },
    SourceRecordStarted {
        id: OperationId,
    },
    SourceRecordFinished {
        id: OperationId,
    },

    BackendSourceBuildQueued {
        id: OperationId,
        package: String,
    },
    BackendSourceBuildStarted {
        id: OperationId,
    },
    BackendSourceBuildFinished {
        id: OperationId,
    },

    InstantiateBackendQueued {
        id: OperationId,
        #[serde(flatten)]
        spec: JsonRpcBackendSpec,
    },
    InstantiateBackendStarted {
        id: OperationId,
    },
    InstantiateBackendFinished {
        id: OperationId,
    },
}

pub struct EventReporter {
    registry: Arc<OperationRegistry>,
    events: EventStore,
}

#[derive(Default, Clone)]
pub struct EventStore(pub(crate) Arc<Mutex<Vec<Event>>>);

impl EventStore {
    pub fn push(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }

    pub async fn wait_until<F>(
        &self,
        mut condition: F,
        timeout: Duration,
    ) -> Result<(), tokio::time::error::Elapsed>
    where
        F: FnMut(&[Event]) -> bool,
    {
        tokio::time::timeout(timeout, async {
            loop {
                {
                    let events = self.0.lock().unwrap();
                    if condition(&events) {
                        break;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
    }

    pub async fn wait_until_matches<F>(
        &self,
        mut condition: F,
        timeout: Duration,
    ) -> Result<(), tokio::time::error::Elapsed>
    where
        F: FnMut(&Event) -> bool,
    {
        self.wait_until(move |events| events.iter().any(&mut condition), timeout)
            .await
    }

    #[allow(dead_code)]
    pub fn contains(&self, condition: impl FnMut(&Event) -> bool) -> bool {
        let events = self.0.lock().unwrap();
        events.iter().any(condition)
    }

    pub fn take(&self) -> Vec<Event> {
        let mut events = self.0.lock().unwrap();
        std::mem::take(&mut *events)
    }
}

impl EventReporter {
    pub fn new() -> (Arc<Self>, EventStore, Arc<OperationRegistry>) {
        let registry = OperationRegistry::new();
        let (reporter, events) = Self::with_registry(registry.clone());
        (reporter, events, registry)
    }

    /// Construct a reporter that allocates ids on a caller-provided
    /// registry. Use this when several reporters need to share one
    /// parent map (e.g., two consecutive dispatcher runs in a single
    /// test merging events into a unified tree).
    pub fn with_registry(registry: Arc<OperationRegistry>) -> (Arc<Self>, EventStore) {
        let events = EventStore::default();
        let reporter = Arc::new(Self {
            registry,
            events: events.clone(),
        });
        (reporter, events)
    }

    fn alloc(&self) -> OperationId {
        self.registry.allocate()
    }

    fn record(&self, event: Event) {
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl CondaSolveReporter for EventReporter {
    fn on_queued(&self, env: &SolveCondaEnvironmentSpec) -> OperationId {
        let id = self.alloc();
        self.record(Event::CondaSolveQueued {
            id,
            spec: Box::new(env.clone()),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::CondaSolveStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::CondaSolveFinished { id });
    }
}

impl PixiSolveReporter for EventReporter {
    fn on_queued(&self, env: &PixiSolveEnvironmentSpec) -> OperationId {
        let id = self.alloc();
        self.record(Event::PixiSolveQueued {
            id,
            spec: env.clone(),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::PixiSolveStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::PixiSolveFinished { id });
    }
}

impl PixiInstallReporter for EventReporter {
    fn on_queued(&self, env: &InstallPixiEnvironmentSpec) -> OperationId {
        let id = self.alloc();
        self.record(Event::PixiInstallQueued {
            id,
            spec: env.clone(),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::PixiInstallStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::PixiInstallFinished { id });
    }
}

impl UrlCheckoutReporter for EventReporter {
    fn on_queued(&self, env: &Url) -> OperationId {
        let id = self.alloc();
        self.record(Event::UrlCheckoutQueued {
            id,
            url: env.clone(),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::UrlCheckoutStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::UrlCheckoutFinished { id });
    }
}

impl GitCheckoutReporter for EventReporter {
    fn on_queued(&self, env: &RepositoryReference) -> OperationId {
        let id = self.alloc();
        self.record(Event::GitCheckoutQueued {
            id,
            reference: env.clone(),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::GitCheckoutStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::GitCheckoutFinished { id });
    }
}

impl BuildBackendMetadataReporter for EventReporter {
    fn on_queued(&self, spec: &BuildBackendMetadataInner) -> OperationId {
        let id = self.alloc();
        self.record(Event::BuildBackendMetadataQueued {
            id,
            spec: spec.clone(),
        });
        id
    }

    fn on_started(
        &self,
        id: OperationId,
        mut backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    ) {
        self.record(Event::BuildBackendMetadataStarted { id });

        tokio::spawn(async move {
            while let Some(line) = backend_output_stream.next().await {
                eprintln!("{line}");
            }
        });
    }

    fn on_finished(&self, id: OperationId, _failed: bool) {
        self.record(Event::BuildBackendMetadataFinished { id });
    }
}

impl SourceMetadataReporter for EventReporter {
    fn on_queued(&self, spec: &SourceMetadataReporterSpec) -> OperationId {
        let id = self.alloc();
        self.record(Event::SourceMetadataQueued {
            id,
            spec: spec.clone(),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::SourceMetadataStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::SourceMetadataFinished { id });
    }
}

impl SourceRecordReporter for EventReporter {
    fn on_queued(&self, spec: &SourceRecordReporterSpec) -> OperationId {
        let id = self.alloc();
        self.record(Event::SourceRecordQueued {
            id,
            spec: spec.clone(),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::SourceRecordStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::SourceRecordFinished { id });
    }
}

impl InstantiateBackendReporter for EventReporter {
    fn on_queued(&self, spec: &JsonRpcBackendSpec) -> OperationId {
        let id = self.alloc();
        self.record(Event::InstantiateBackendQueued {
            id,
            spec: spec.clone(),
        });
        id
    }

    fn on_started(&self, id: OperationId) {
        self.record(Event::InstantiateBackendStarted { id });
    }

    fn on_finished(&self, id: OperationId) {
        self.record(Event::InstantiateBackendFinished { id });
    }
}

impl BackendSourceBuildReporter for EventReporter {
    fn on_queued(&self, spec: &BackendSourceBuildSpec) -> OperationId {
        let id = self.alloc();
        self.record(Event::BackendSourceBuildQueued {
            id,
            package: spec.name.as_source().to_string(),
        });
        id
    }

    fn on_started(
        &self,
        id: OperationId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    ) {
        self.record(Event::BackendSourceBuildStarted { id });

        tokio::spawn(async move {
            let mut output_stream = backend_output_stream;
            while let Some(line) = output_stream.next().await {
                eprintln!("{line}");
            }
        });
    }

    fn on_finished(&self, id: OperationId, _failed: bool) {
        self.record(Event::BackendSourceBuildFinished { id });
    }
}

impl EventReporter {
    /// Register this event reporter as every per-key reporter the
    /// dispatcher knows about. The same Arc is shared across all
    /// sub-reporter roles so events collect into one queue.
    pub fn register_with(
        self: Arc<Self>,
        builder: pixi_command_dispatcher::CommandDispatcherBuilder,
    ) -> pixi_command_dispatcher::CommandDispatcherBuilder {
        builder
            .with_git_checkout_reporter(self.clone())
            .with_url_checkout_reporter(self.clone())
            .with_conda_solve_reporter(self.clone())
            .with_pixi_solve_reporter(self.clone())
            .with_pixi_install_reporter(self.clone())
            .with_instantiate_backend_reporter(self.clone())
            .with_build_backend_metadata_reporter(self.clone())
            .with_source_metadata_reporter(self.clone())
            .with_source_record_reporter(self.clone())
            .with_backend_source_build_reporter(self.clone())
    }
}

/// Test-only ext on [`pixi_command_dispatcher::CommandDispatcherBuilder`]
/// that registers an [`EventReporter`] under all per-key reporter slots
/// in one chained call.
pub(crate) trait WithEventReporter {
    fn with_event_reporter(self, reporter: Arc<EventReporter>) -> Self;
}

impl WithEventReporter for pixi_command_dispatcher::CommandDispatcherBuilder {
    fn with_event_reporter(self, reporter: Arc<EventReporter>) -> Self {
        reporter.register_with(self)
    }
}
