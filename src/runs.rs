use chrono::{DateTime, Local, Utc};
use daemonize::Daemonize;
use miette::IntoDiagnostic;
use serde::{Deserialize, Serialize, Serializer};
use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
};
use sysinfo::{Pid, Process, ProcessStatus, System};

use crate::{consts, Project};

#[derive(Debug)]
pub struct DaemonRunsManager<'a> {
    pub project: &'a Project,
}

impl<'a> DaemonRunsManager<'a> {
    pub fn new(project: &'a Project) -> Self {
        let daemon_runs = Self { project };

        // Create the runs directory if it doesn't exist
        std::fs::create_dir_all(daemon_runs.runs_dir()).expect("Failed to create runs directory");

        daemon_runs
    }

    pub fn runs_dir(&self) -> PathBuf {
        self.project.pixi_dir().join(consts::RUNS_DIR)
    }

    pub fn runs(&self) -> Vec<DaemonRun> {
        // NOTE(hadim): the source of truth for managed runs are the pid files (any files ending with `.pid`) in the runs directory.

        let runs: Vec<DaemonRun> = std::fs::read_dir(self.runs_dir())
            .expect("Failed to read runs directory")
            .filter_map(|entry| {
                let entry = entry.expect("Failed to read entry");
                let path = entry.path();
                let file_name = path.file_name().expect("Failed to get file name");
                let file_name = file_name
                    .to_str()
                    .expect("Failed to convert file name to str");
                if file_name.ends_with(".pid") {
                    let run_name = file_name.replace(".pid", "").into();

                    Some(DaemonRun::new(
                        run_name,
                        self.runs_dir(),
                        self.project.root().to_path_buf(),
                    ))
                } else {
                    None
                }
            })
            .collect();

        runs
    }

    pub fn create_new_run(&self, name: Option<String>) -> miette::Result<DaemonRun> {
        // Check if a run with the same name already exists
        let name = match name {
            Some(name) => name,
            None => miette::bail!("You must provide a name for the run."),
        };

        // check not the same name as an existing run
        if self.runs().iter().any(|run| run.name == name) {
            miette::bail!("A run with the same name already exists. You can call `pixi runs clear` to clear all the terminated runs.");
        }

        Ok(DaemonRun::new(
            name,
            self.runs_dir(),
            self.project.root().to_path_buf(),
        ))
    }

    pub fn get_run(&self, name: String) -> miette::Result<DaemonRun> {
        let run = DaemonRun::new(name, self.runs_dir(), self.project.root().to_path_buf());

        // Check the pid file exists
        if !run.pid_file_path().exists() {
            miette::bail!("No run with name '{}' found.", run.name);
        }

        Ok(run)
    }
}

#[derive(Debug)]
pub struct DaemonRun {
    pub name: String,
    pub runs_dir: PathBuf,
    pub working_dir: PathBuf,
}

impl DaemonRun {
    pub fn new(name: String, runs_dir: PathBuf, working_dir: PathBuf) -> Self {
        Self {
            name,
            runs_dir,
            working_dir,
        }
    }

    pub fn is_running(&self) -> bool {
        let pid = match self.read_pid() {
            Some(pid) => pid,
            None => return false,
        };

        // TODO: not very efficient to call this every time
        let mut system = System::new_all();
        system.refresh_all();

        match system.process(pid) {
            Some(process) => process.status() == sysinfo::ProcessStatus::Run,
            None => false,
        }
    }

    pub fn process_status(&self) -> DaemonRunStatus {
        let pid = match self.read_pid() {
            Some(pid) => pid,
            None => return DaemonRunStatus::Unknown,
        };

        // TODO: not very efficient to call this every time
        let mut system = System::new_all();
        system.refresh_all();

        match system.process(pid) {
            Some(process) => DaemonRunStatus::ProcessStatus(process.status()),
            None => DaemonRunStatus::Terminated,
        }
    }

    pub fn pid_file_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.pid", self.name))
    }

    pub fn stdout_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.out", self.name))
    }

    pub fn stderr_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.err", self.name))
    }

    pub fn infos_file_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.infos.json", self.name))
    }

    pub fn read_pid(&self) -> Option<Pid> {
        if !self.pid_file_path().exists() {
            return None;
        }

        let pid = std::fs::read_to_string(self.pid_file_path())
            .expect("Failed to read pid file")
            .trim()
            .parse::<Pid>()
            .expect("Failed to parse pid file content as u32");

        Some(pid)
    }

    pub fn read_infos(&self) -> Option<DaemonRunInfos> {
        if !self.infos_file_path().exists() {
            return None;
        }

        let infos_json = std::fs::read_to_string(self.infos_file_path()).unwrap();
        let infos: DaemonRunInfos = serde_json::from_str(&infos_json).unwrap();

        Some(infos)
    }

    pub fn read_stdout(&self) -> String {
        std::fs::read_to_string(self.stdout_path()).unwrap()
    }

    pub fn read_stderr(&self) -> String {
        std::fs::read_to_string(self.stderr_path()).unwrap()
    }

    pub fn start(&self, task: Vec<String>) -> miette::Result<()> {
        // Create stdout and stderr files
        let stdout = std::fs::File::create(self.stdout_path()).unwrap();
        let stderr = std::fs::File::create(self.stderr_path()).unwrap();

        // Create and save the infos file
        let infos = DaemonRunInfos {
            name: self.name.clone(),
            task,
            start_date: Local::now(),
        };
        let infos_json = serde_json::to_string_pretty(&infos).unwrap();
        std::fs::write(self.infos_file_path(), infos_json).unwrap();

        // Create the daemon
        let daemonize = Daemonize::new()
            .pid_file(self.pid_file_path())
            .stdout(stdout)
            .stderr(stderr)
            .umask(0o027) // Set umask, `0o027` by default.
            .chown_pid_file(true)
            .working_directory(self.working_dir.clone());

        // Start the daemon
        daemonize.start().into_diagnostic()
    }

    pub fn clear(&self) -> miette::Result<()> {
        // check if the run is running
        if self.is_running() {
            miette::bail!("The run is still running. You can call `pixi runs kill` to kill it.");
        }

        // delete pid, infos, stdout and stderr files
        std::fs::remove_file(self.pid_file_path()).expect("Failed to remove pid file");
        std::fs::remove_file(self.infos_file_path()).expect("Failed to remove infos file");
        std::fs::remove_file(self.stdout_path()).expect("Failed to remove stdout file");
        std::fs::remove_file(self.stderr_path()).expect("Failed to remove stderr file");

        Ok(())
    }

    pub fn state(&self) -> miette::Result<DaemonRunState> {
        let pid = match self.read_pid() {
            Some(pid) => pid,
            None => miette::bail!("No pid file with name '{}' found.", self.name),
        };
        let stdout_length = std::fs::read_to_string(self.stdout_path()).unwrap().len() as usize;
        let stderr_length = std::fs::read_to_string(self.stderr_path()).unwrap().len() as usize;
        let infos = match self.read_infos() {
            Some(infos) => infos,
            None => miette::bail!("No infos file with name '{}' found.", self.name),
        };

        Ok(DaemonRunState {
            name: infos.name,
            status: self.process_status(),
            pid: pid.as_u32(),
            task: infos.task,
            start_date: infos.start_date,
            stdout_length: stdout_length,
            stderr_length: stderr_length,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRunInfos {
    pub name: String,
    pub task: Vec<String>,
    pub start_date: DateTime<Local>,
}

#[derive(Debug)]
pub struct DaemonRunState {
    pub name: String,
    pub status: DaemonRunStatus,
    pub pid: u32,
    pub start_date: DateTime<Local>,
    pub task: Vec<String>,
    pub stdout_length: usize,
    pub stderr_length: usize,
}

#[derive(Debug)]
pub enum DaemonRunStatus {
    Terminated,
    Unknown,
    ProcessStatus(ProcessStatus),
}

impl DaemonRunStatus {
    pub fn from_string(status_str: String) -> miette::Result<DaemonRunStatus> {
        let status = match status_str.as_str() {
            "Terminated" => DaemonRunStatus::Terminated,
            "Unknown" => DaemonRunStatus::Unknown,
            "Idle" => DaemonRunStatus::ProcessStatus(ProcessStatus::Idle),
            "Run" => DaemonRunStatus::ProcessStatus(ProcessStatus::Run),
            "Sleep" => DaemonRunStatus::ProcessStatus(ProcessStatus::Sleep),
            "Stop" => DaemonRunStatus::ProcessStatus(ProcessStatus::Stop),
            "Zombie" => DaemonRunStatus::ProcessStatus(ProcessStatus::Zombie),
            "Tracing" => DaemonRunStatus::ProcessStatus(ProcessStatus::Tracing),
            "Dead" => DaemonRunStatus::ProcessStatus(ProcessStatus::Dead),
            "Wakekill" => DaemonRunStatus::ProcessStatus(ProcessStatus::Wakekill),
            "Waking" => DaemonRunStatus::ProcessStatus(ProcessStatus::Waking),
            "Parked" => DaemonRunStatus::ProcessStatus(ProcessStatus::Parked),
            "LockBlocked" => DaemonRunStatus::ProcessStatus(ProcessStatus::LockBlocked),
            "UninterruptibleDiskSleep" => {
                DaemonRunStatus::ProcessStatus(ProcessStatus::UninterruptibleDiskSleep)
            }
            "UnknownFromSys" => DaemonRunStatus::ProcessStatus(ProcessStatus::Unknown(0)),
            _ => miette::bail!("Unknown status '{}'", status_str),
        };

        Ok(status)
    }
}

// implement tostring
impl ToString for DaemonRunStatus {
    fn to_string(&self) -> String {
        match self {
            DaemonRunStatus::Terminated => "Terminated".to_string(),
            DaemonRunStatus::Unknown => "Unknown".to_string(),
            DaemonRunStatus::ProcessStatus(status) => match status {
                ProcessStatus::Idle => "Idle".to_string(),
                ProcessStatus::Run => "Run".to_string(),
                ProcessStatus::Sleep => "Sleep".to_string(),
                ProcessStatus::Stop => "Stop".to_string(),
                ProcessStatus::Zombie => "Zombie".to_string(),
                ProcessStatus::Tracing => "Tracing".to_string(),
                ProcessStatus::Dead => "Dead".to_string(),
                ProcessStatus::Wakekill => "Wakekill".to_string(),
                ProcessStatus::Waking => "Waking".to_string(),
                ProcessStatus::Parked => "Parked".to_string(),
                ProcessStatus::LockBlocked => "LockBlocked".to_string(),
                ProcessStatus::UninterruptibleDiskSleep => "UninterruptibleDiskSleep".to_string(),
                ProcessStatus::Unknown(_) => "UnknownFromSys".to_string(),
            },
        }
    }
}

// // implement display
// impl Display for DaemonRunStatus {
//     fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
//         write!(f, "{}", self.to_string())
//     }
// }

// // implement serialize
// impl Serialize for DaemonRunStatus {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: Serializer,
//     {
//         DaemonRunStatus::to_string(self).serialize(serializer)
//     }
// }

// // implement deserialize
// impl<'de> Deserialize<'de> for DaemonRunStatus {
//     fn deserialize<D>(deserializer: D) -> Result<DaemonRunStatus, D::Error>
//     where
//         D: serde::Deserializer<'de>,
//     {
//         let status_str = String::deserialize(deserializer)?;
//         DaemonRunStatus::from_string(status_str).map_err(serde::de::Error::custom)
//     }
// }
