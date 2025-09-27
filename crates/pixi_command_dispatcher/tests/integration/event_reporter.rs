use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use futures::{Stream, StreamExt};
use pixi_command_dispatcher::{
    BackendSourceBuildSpec, BuildBackendMetadataSpec, CondaSolveReporter, GitCheckoutReporter,
    InstallPixiEnvironmentSpec, InstantiateToolEnvironmentSpec, PackageIdentifier,
    PixiEnvironmentSpec, PixiInstallReporter, PixiSolveReporter, Reporter, ReporterContext,
    SolveCondaEnvironmentSpec, SourceBuildSpec, SourceMetadataSpec,
    reporter::{
        BackendSourceBuildId, BackendSourceBuildReporter, BuildBackendMetadataId,
        BuildBackendMetadataReporter, CondaSolveId, GitCheckoutId, InstantiateToolEnvId,
        InstantiateToolEnvironmentReporter, PixiInstallId, PixiSolveId, SourceBuildId,
        SourceBuildReporter, SourceMetadataId, SourceMetadataReporter,
    },
};
use pixi_git::resolver::RepositoryReference;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    CondaSolveQueued {
        id: CondaSolveId,
        #[serde(flatten)]
        spec: Box<SolveCondaEnvironmentSpec>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    CondaSolveStarted {
        id: CondaSolveId,
    },
    CondaSolveFinished {
        id: CondaSolveId,
    },

    PixiSolveQueued {
        id: PixiSolveId,
        #[serde(flatten)]
        spec: PixiEnvironmentSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    PixiSolveStarted {
        id: PixiSolveId,
    },
    PixiSolveFinished {
        id: PixiSolveId,
    },

    PixiInstallQueued {
        id: PixiInstallId,
        #[serde(flatten)]
        spec: InstallPixiEnvironmentSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    PixiInstallStarted {
        id: PixiInstallId,
    },
    PixiInstallFinished {
        id: PixiInstallId,
    },

    GitCheckoutQueued {
        id: GitCheckoutId,
        #[serde(flatten)]
        reference: RepositoryReference,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    GitCheckoutStarted {
        id: GitCheckoutId,
    },
    GitCheckoutFinished {
        id: GitCheckoutId,
    },

    BuildBackendMetadataQueued {
        id: BuildBackendMetadataId,
        #[serde(flatten)]
        spec: BuildBackendMetadataSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    BuildBackendMetadataStarted {
        id: BuildBackendMetadataId,
    },
    BuildBackendMetadataFinished {
        id: BuildBackendMetadataId,
    },

    SourceMetadataQueued {
        id: SourceMetadataId,
        #[serde(flatten)]
        spec: SourceMetadataSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    SourceMetadataStarted {
        id: SourceMetadataId,
    },
    SourceMetadataFinished {
        id: SourceMetadataId,
    },

    SourceBuildQueued {
        id: SourceBuildId,
        #[serde(flatten)]
        spec: SourceBuildSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    SourceBuildStarted {
        id: SourceBuildId,
    },
    SourceBuildFinished {
        id: SourceBuildId,
    },

    BackendSourceBuildQueued {
        id: BackendSourceBuildId,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
        package: PackageIdentifier,
    },
    BackendSourceBuildStarted {
        id: BackendSourceBuildId,
    },
    BackendSourceBuildFinished {
        id: BackendSourceBuildId,
    },

    InstantiateToolEnvQueued {
        id: InstantiateToolEnvId,
        #[serde(flatten)]
        spec: InstantiateToolEnvironmentSpec,
        #[serde(skip_serializing_if = "Option::is_none")]
        context: Option<ReporterContext>,
    },
    InstantiateToolEnvStarted {
        id: InstantiateToolEnvId,
    },
    InstantiateToolEnvFinished {
        id: InstantiateToolEnvId,
    },
}

pub struct EventReporter {
    events: EventStore,
    next_conda_solve_id: usize,
    next_pixi_solve_id: usize,
    next_pixi_install_id: usize,
    next_git_checkout_id: usize,
    next_source_metadata_id: usize,
    next_instantiate_tool_env_id: usize,
}

#[derive(Default, Clone)]
pub struct EventStore(pub(crate) Arc<Mutex<Vec<Event>>>);

impl EventStore {
    /// Push a new event to the store.
    pub fn push(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }

    /// Wait for a certain condition to hold.
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

    /// Wait until the store contains an event that matches the condition.
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

    /// Returns true if the store contains an event that matches the condition.
    pub fn contains(&self, condition: impl FnMut(&Event) -> bool) -> bool {
        let events = self.0.lock().unwrap();
        events.iter().any(condition)
    }

    /// Takes all events from the store, leaving it empty.
    pub fn take(&self) -> Vec<Event> {
        let mut events = self.0.lock().unwrap();
        std::mem::take(&mut *events)
    }
}

impl EventReporter {
    pub fn new() -> (Self, EventStore) {
        let events = EventStore::default();
        (
            Self {
                events: events.clone(),
                next_conda_solve_id: 0,
                next_pixi_solve_id: 0,
                next_pixi_install_id: 0,
                next_git_checkout_id: 0,
                next_source_metadata_id: 0,
                next_instantiate_tool_env_id: 0,
            },
            events,
        )
    }
}

impl CondaSolveReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        env: &SolveCondaEnvironmentSpec,
    ) -> CondaSolveId {
        let next_id = CondaSolveId(self.next_conda_solve_id);
        self.next_conda_solve_id += 1;

        let event = Event::CondaSolveQueued {
            id: next_id,
            spec: Box::new(env.clone()),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_start(&mut self, solve_id: CondaSolveId) {
        let event = Event::CondaSolveStarted { id: solve_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, solve_id: CondaSolveId) {
        let event = Event::CondaSolveFinished { id: solve_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl PixiSolveReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        env: &PixiEnvironmentSpec,
    ) -> PixiSolveId {
        let next_id = PixiSolveId(self.next_pixi_solve_id);
        self.next_pixi_solve_id += 1;

        let event = Event::PixiSolveQueued {
            id: next_id,
            spec: env.clone(),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_start(&mut self, solve_id: PixiSolveId) {
        let event = Event::PixiSolveStarted { id: solve_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, solve_id: PixiSolveId) {
        let event = Event::PixiSolveFinished { id: solve_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl PixiInstallReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        env: &InstallPixiEnvironmentSpec,
    ) -> PixiInstallId {
        let next_id = PixiInstallId(self.next_pixi_install_id);
        self.next_pixi_install_id += 1;

        let event = Event::PixiInstallQueued {
            id: next_id,
            spec: env.clone(),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_start(&mut self, solve_id: PixiInstallId) {
        let event = Event::PixiInstallStarted { id: solve_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, solve_id: PixiInstallId) {
        let event = Event::PixiInstallFinished { id: solve_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl GitCheckoutReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        env: &RepositoryReference,
    ) -> GitCheckoutId {
        let next_id = GitCheckoutId(self.next_git_checkout_id);
        self.next_git_checkout_id += 1;

        let event = Event::GitCheckoutQueued {
            id: next_id,
            reference: env.clone(),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_start(&mut self, checkout_id: GitCheckoutId) {
        let event = Event::GitCheckoutStarted { id: checkout_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, checkout_id: GitCheckoutId) {
        let event = Event::GitCheckoutFinished { id: checkout_id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl BuildBackendMetadataReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        spec: &BuildBackendMetadataSpec,
    ) -> BuildBackendMetadataId {
        let next_id = BuildBackendMetadataId(self.next_source_metadata_id);
        self.next_source_metadata_id += 1;

        let event = Event::BuildBackendMetadataQueued {
            id: next_id,
            spec: spec.clone(),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_started(&mut self, id: BuildBackendMetadataId) {
        let event = Event::BuildBackendMetadataStarted { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, id: BuildBackendMetadataId) {
        let event = Event::BuildBackendMetadataFinished { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl SourceMetadataReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        spec: &SourceMetadataSpec,
    ) -> SourceMetadataId {
        let next_id = SourceMetadataId(self.next_source_metadata_id);
        self.next_source_metadata_id += 1;

        let event = Event::SourceMetadataQueued {
            id: next_id,
            spec: spec.clone(),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_started(&mut self, id: SourceMetadataId) {
        let event = Event::SourceMetadataStarted { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, id: SourceMetadataId) {
        let event = Event::SourceMetadataFinished { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl InstantiateToolEnvironmentReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        spec: &InstantiateToolEnvironmentSpec,
    ) -> InstantiateToolEnvId {
        let next_id = InstantiateToolEnvId(self.next_instantiate_tool_env_id);
        self.next_instantiate_tool_env_id += 1;

        let event = Event::InstantiateToolEnvQueued {
            id: next_id,
            spec: spec.clone(),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_started(&mut self, id: InstantiateToolEnvId) {
        let event = Event::InstantiateToolEnvStarted { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, id: InstantiateToolEnvId) {
        let event = Event::InstantiateToolEnvFinished { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl SourceBuildReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        spec: &SourceBuildSpec,
    ) -> SourceBuildId {
        let next_id = SourceBuildId(self.next_source_metadata_id);
        self.next_source_metadata_id += 1;

        let event = Event::SourceBuildQueued {
            id: next_id,
            spec: spec.clone(),
            context,
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_started(&mut self, id: SourceBuildId) {
        let event = Event::SourceBuildStarted { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }

    fn on_finished(&mut self, id: SourceBuildId) {
        let event = Event::SourceBuildFinished { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl BackendSourceBuildReporter for EventReporter {
    fn on_queued(
        &mut self,
        context: Option<ReporterContext>,
        spec: &BackendSourceBuildSpec,
    ) -> BackendSourceBuildId {
        let next_id = BackendSourceBuildId(self.next_source_metadata_id);
        self.next_source_metadata_id += 1;

        let event = Event::BackendSourceBuildQueued {
            id: next_id,
            context,
            package: spec.package.clone(),
        };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
        next_id
    }

    fn on_started(
        &mut self,
        id: BackendSourceBuildId,
        backend_output_stream: Box<dyn Stream<Item = String> + Unpin + Send>,
    ) {
        let event = Event::BackendSourceBuildStarted { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);

        tokio::spawn(async move {
            let mut output_stream = backend_output_stream;
            while let Some(line) = output_stream.next().await {
                eprintln!("{}", line);
            }
        });
    }

    fn on_finished(&mut self, id: BackendSourceBuildId, _failed: bool) {
        let event = Event::BackendSourceBuildFinished { id };
        eprintln!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.push(event);
    }
}

impl Reporter for EventReporter {
    fn as_git_reporter(&mut self) -> Option<&mut dyn GitCheckoutReporter> {
        Some(self)
    }

    fn as_conda_solve_reporter(&mut self) -> Option<&mut dyn CondaSolveReporter> {
        Some(self)
    }

    fn as_pixi_solve_reporter(&mut self) -> Option<&mut dyn PixiSolveReporter> {
        Some(self)
    }

    fn as_pixi_install_reporter(&mut self) -> Option<&mut dyn PixiInstallReporter> {
        Some(self)
    }

    fn as_instantiate_tool_environment_reporter(
        &mut self,
    ) -> Option<&mut dyn InstantiateToolEnvironmentReporter> {
        Some(self)
    }

    fn as_build_backend_metadata_reporter(
        &mut self,
    ) -> Option<&mut dyn BuildBackendMetadataReporter> {
        Some(self)
    }
    fn as_source_metadata_reporter(&mut self) -> Option<&mut dyn SourceMetadataReporter> {
        Some(self)
    }
    fn as_source_build_reporter(&mut self) -> Option<&mut dyn SourceBuildReporter> {
        Some(self)
    }

    fn as_backend_source_build_reporter(&mut self) -> Option<&mut dyn BackendSourceBuildReporter> {
        Some(self)
    }
}
