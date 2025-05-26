use std::sync::{Arc, Mutex};

use pixi_command_dispatcher::{
    CondaSolveReporter, GitCheckoutReporter, InstallPixiEnvironmentSpec, PixiEnvironmentSpec,
    PixiInstallReporter, PixiSolveReporter, Reporter, ReporterContext, SolveCondaEnvironmentSpec,
    reporter::{CondaSolveId, GitCheckoutId, PixiInstallId, PixiSolveId},
};
use pixi_git::resolver::RepositoryReference;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum EventType {
    CondaSolveQueued { id: CondaSolveId },
    CondaSolveStarted { id: CondaSolveId },
    CondaSolveFinished { id: CondaSolveId },
    PixiSolveQueued { id: PixiSolveId },
    PixiSolveStarted { id: PixiSolveId },
    PixiSolveFinished { id: PixiSolveId },
    PixiInstallQueued { id: PixiInstallId },
    PixiInstallStarted { id: PixiInstallId },
    PixiInstallFinished { id: PixiInstallId },
    GitCheckoutQueued { id: GitCheckoutId },
    GitCheckoutStarted { id: GitCheckoutId },
    GitCheckoutFinished { id: GitCheckoutId },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    CondaSolveQueued {
        id: CondaSolveId,
        #[serde(flatten)]
        spec: SolveCondaEnvironmentSpec,
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
}

impl Event {
    pub fn event_type(&self) -> EventType {
        match self {
            Event::CondaSolveQueued { id, .. } => EventType::CondaSolveQueued { id: *id },
            Event::CondaSolveStarted { id, .. } => EventType::CondaSolveStarted { id: *id },
            Event::CondaSolveFinished { id, .. } => EventType::CondaSolveFinished { id: *id },
            Event::PixiSolveQueued { id, .. } => EventType::PixiSolveQueued { id: *id },
            Event::PixiSolveStarted { id, .. } => EventType::PixiSolveStarted { id: *id },
            Event::PixiSolveFinished { id, .. } => EventType::PixiSolveFinished { id: *id },
            Event::PixiInstallQueued { id, .. } => EventType::PixiInstallQueued { id: *id },
            Event::PixiInstallStarted { id, .. } => EventType::PixiInstallStarted { id: *id },
            Event::PixiInstallFinished { id, .. } => EventType::PixiInstallFinished { id: *id },
            Event::GitCheckoutQueued { id, .. } => EventType::GitCheckoutQueued { id: *id },
            Event::GitCheckoutStarted { id, .. } => EventType::GitCheckoutStarted { id: *id },
            Event::GitCheckoutFinished { id, .. } => EventType::GitCheckoutFinished { id: *id },
        }
    }
}

pub struct EventReporter {
    events: Arc<Mutex<Vec<Event>>>,
    next_conda_solve_id: usize,
    next_pixi_solve_id: usize,
    next_pixi_install_id: usize,
    next_git_checkout_id: usize,
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
            },
            events,
        )
    }
}

impl CondaSolveReporter for EventReporter {
    fn on_solve_queued(
        &mut self,
        context: Option<ReporterContext>,
        env: &SolveCondaEnvironmentSpec,
    ) -> CondaSolveId {
        let next_id = CondaSolveId(self.next_conda_solve_id);
        self.next_conda_solve_id += 1;

        let event = Event::CondaSolveQueued {
            id: next_id,
            spec: env.clone(),
            context,
        };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
        next_id
    }

    fn on_solve_start(&mut self, solve_id: CondaSolveId) {
        let event = Event::CondaSolveStarted { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_solve_finished(&mut self, solve_id: CondaSolveId) {
        let event = Event::CondaSolveFinished { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }
}

impl PixiSolveReporter for EventReporter {
    fn on_solve_queued(
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

    fn on_solve_start(&mut self, solve_id: PixiSolveId) {
        let event = Event::PixiSolveStarted { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_solve_finished(&mut self, solve_id: PixiSolveId) {
        let event = Event::PixiSolveFinished { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }
}

impl PixiInstallReporter for EventReporter {
    fn on_install_queued(
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

    fn on_install_start(&mut self, solve_id: PixiInstallId) {
        let event = Event::PixiInstallStarted { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_install_finished(&mut self, solve_id: PixiInstallId) {
        let event = Event::PixiInstallFinished { id: solve_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }
}

impl GitCheckoutReporter for EventReporter {
    fn on_checkout_queued(
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

    fn on_checkout_start(&mut self, checkout_id: GitCheckoutId) {
        let event = Event::GitCheckoutStarted { id: checkout_id };
        println!("{}", serde_json::to_string_pretty(&event).unwrap());
        self.events.lock().unwrap().push(event);
    }

    fn on_checkout_finished(&mut self, checkout_id: GitCheckoutId) {
        let event = Event::GitCheckoutFinished { id: checkout_id };
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
}
