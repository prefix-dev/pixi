use std::path::PathBuf;

use crate::{consts, Project};

pub struct DaemonRuns {
    pub project: Project,
}

impl DaemonRuns {
    pub fn new(project: Project) -> Self {
        Self { project }
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.project.pixi_dir().join(consts::RUNS_DIR)
    }

    pub fn create_new_run(&self, run_name: String) -> DaemonRun {
        DaemonRun { name: run_name }
    }
}

pub struct DaemonRun {
    pub name: String,
}

// impl DaemonRun {
//     pub fn pid_file(&self) -> PathBuf {
//         runs_dir.join(&self.name)
//     }
// }
