use std::sync::{Arc, Mutex};

use pixi_command_dispatcher::{
    CondaSolveReporter, GitCheckoutReporter, InstallPixiEnvironmentSpec,
    InstantiateToolEnvironmentSpec, PixiEnvironmentSpec, PixiInstallReporter, PixiSolveReporter,
    Reporter, ReporterContext, SolveCondaEnvironmentSpec, SourceMetadataSpec,
    reporter::{
        CondaSolveId, GitCheckoutId, InstantiateToolEnvId, InstantiateToolEnvironmentReporter,
        PixiInstallId, PixiSolveId, SourceMetadataId, SourceMetadataReporter,
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
    events: Arc<Mutex<Vec<Event>>>,
    next_conda_solve_id: usize,
    next_pixi_solve_id: usize,
    next_pixi_install_id: usize,
    next_git_checkout_id: usize,
    next_source_metadata_id: usize,
    next_instantiate_tool_env_id: usize,
}

impl EventReporter {
    pub fn new() -> (Self, Arc<Mutex<Vec<Event>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
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
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
        next_id
    }

    fn on_start(&mut self, solve_id: CondaSolveId) {
        let event = Event::CondaSolveStarted { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_finished(&mut self, solve_id: CondaSolveId) {
        let event = Event::CondaSolveFinished { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
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
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
        next_id
    }

    fn on_start(&mut self, solve_id: PixiSolveId) {
        let event = Event::PixiSolveStarted { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_finished(&mut self, solve_id: PixiSolveId) {
        let event = Event::PixiSolveFinished { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
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
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
        next_id
    }

    fn on_start(&mut self, solve_id: PixiInstallId) {
        let event = Event::PixiInstallStarted { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_finished(&mut self, solve_id: PixiInstallId) {
        let event = Event::PixiInstallFinished { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
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
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
        next_id
    }

    fn on_start(&mut self, checkout_id: GitCheckoutId) {
        let event = Event::GitCheckoutStarted { id: checkout_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_finished(&mut self, checkout_id: GitCheckoutId) {
        let event = Event::GitCheckoutFinished { id: checkout_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
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
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
        next_id
    }

    fn on_started(&mut self, id: SourceMetadataId) {
        let event = Event::SourceMetadataStarted { id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_finished(&mut self, id: SourceMetadataId) {
        let event = Event::SourceMetadataFinished { id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
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
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
        next_id
    }

    fn on_started(&mut self, id: InstantiateToolEnvId) {
        let event = Event::InstantiateToolEnvStarted { id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_finished(&mut self, id: InstantiateToolEnvId) {
        let event = Event::InstantiateToolEnvFinished { id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
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

    fn as_source_metadata_reporter(&mut self) -> Option<&mut dyn SourceMetadataReporter> {
        Some(self)
    }
}
