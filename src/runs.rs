use chrono::{DateTime, Local};
#[cfg(unix)]
use daemonize::Daemonize;
use miette::IntoDiagnostic;
use names::Generator;
use serde::{Deserialize, Serialize};
use std::{fmt, path::PathBuf};
use sysinfo::{Pid, ProcessStatus, System};

use crate::{consts, Project};

/// Manage the daemon (detached) runs of a project.
#[derive(Debug)]
pub struct DaemonRunsManager<'a> {
    pub project: &'a Project,
}

impl<'a> DaemonRunsManager<'a> {
    /// Create a new `DaemonRunsManager` for a project.
    /// This will create the runs directory if it doesn't exist.
    pub fn new(project: &'a Project) -> Self {
        let daemon_runs = Self { project };

        // Create the runs directory if it doesn't exist
        std::fs::create_dir_all(daemon_runs.runs_dir()).expect("Failed to create runs directory");

        daemon_runs
    }

    /// Get the runs directory of the project.
    pub fn runs_dir(&self) -> PathBuf {
        self.project.pixi_dir().join(consts::RUNS_DIR)
    }

    /// Get the runs of the project. The source of truth for managed runs are the pid files (any files ending with `.pid`) in the runs directory.
    pub fn runs(&self) -> Vec<DaemonRun> {
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
                    let run_name = file_name.replace(".pid", "");

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

    /// Create a new run for the project. Runs with the same name are not allowed. If no name
    /// is provided, a random name will be generated.
    pub fn create_new_run(&self, name: Option<String>) -> miette::Result<DaemonRun> {
        // Check if a run with the same name already exists
        let name = match name {
            Some(name) => name,
            None => {
                // We generate a random name for this run
                let mut generator = Generator::default();
                generator.next().expect("Failed to generate random name")
            }
        };

        // Check not the same name as an existing run
        if self.runs().iter().any(|run| run.name == name) {
            miette::bail!("A run with the same name already exists. You can call `pixi runs clear` to clear all the terminated runs.");
        }

        Ok(DaemonRun::new(
            name,
            self.runs_dir(),
            self.project.root().to_path_buf(),
        ))
    }

    /// Get a run by its name.
    pub fn get_run(&self, name: String) -> miette::Result<DaemonRun> {
        let run = DaemonRun::new(name, self.runs_dir(), self.project.root().to_path_buf());

        // Check the pid file exists
        if !run.pid_file_path().exists() {
            miette::bail!("No run with name '{}' found.", run.name);
        }

        Ok(run)
    }
}

/// A detached run of a project.
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

    /// Check if the run is alive. A run is considered alive if its PID can be found in the
    /// system using the `sysinfo` crate.
    pub fn is_alive(&self) -> bool {
        !matches!(
            self.process_status(),
            DaemonRunStatus::Terminated | DaemonRunStatus::UnknownPid
        )
    }

    /// Get the status of the run. If the run is not alive, the status is either `Terminated` or
    /// `UnknownPid`.
    pub fn process_status(&self) -> DaemonRunStatus {
        let pid = match self.read_pid() {
            Some(pid) => pid,
            None => return DaemonRunStatus::UnknownPid,
        };

        // `SystemInfo::refresh_system()` must have been call before.
        let system = SystemInfo::get();

        match system.process(pid) {
            Some(process) => DaemonRunStatus::from_process_status(process.status()),
            // if no process is associated with the pid, it means the process is terminated
            None => DaemonRunStatus::Terminated,
        }
    }

    /// Get the path to the pid file of the run.
    pub fn pid_file_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.pid", self.name))
    }

    /// Get the path to the stdout file of the run.
    pub fn stdout_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.out", self.name))
    }

    /// Get the path to the stderr file of the run.
    pub fn stderr_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.err", self.name))
    }

    /// Get the path to the infos file of the run.
    pub fn infos_file_path(&self) -> PathBuf {
        self.runs_dir.join(format!("{}.infos.json", self.name))
    }

    /// Read the pid of the run from the pid file.
    pub fn read_pid(&self) -> Option<Pid> {
        if !self.pid_file_path().exists() {
            return None;
        }

        let pid = match std::fs::read_to_string(self.pid_file_path()) {
            Ok(content) => content.trim().parse::<Pid>().ok(),
            Err(_) => None,
        };

        pid
    }

    /// Read the infos of the run from the infos file.
    pub fn read_infos(&self) -> Option<DaemonRunInfos> {
        if !self.infos_file_path().exists() {
            return None;
        }

        let infos_json = match std::fs::read_to_string(self.infos_file_path()) {
            Ok(json) => json,
            Err(_) => return None,
        };

        match serde_json::from_str(&infos_json) {
            Ok(infos) => Some(infos),
            Err(_) => None,
        }
    }

    /// Read the stdout of the run from the stdout file.
    pub fn read_stdout(&self) -> miette::Result<String> {
        std::fs::read_to_string(self.stdout_path()).into_diagnostic()
    }

    /// Read the stderr of the run from the stderr file.
    pub fn read_stderr(&self) -> miette::Result<String> {
        std::fs::read_to_string(self.stderr_path()).into_diagnostic()
    }

    /// Start a daemon for the run. This will create the pid, stdout, stderr and infos files.
    pub fn start(&self, task: Vec<String>) -> miette::Result<()> {
        #[cfg(unix)]
        {
            // Create stdout and stderr files
            let stdout = std::fs::File::create(self.stdout_path()).into_diagnostic()?;
            let stderr = std::fs::File::create(self.stderr_path()).into_diagnostic()?;

            // Create and save the infos file
            let infos = DaemonRunInfos {
                name: self.name.clone(),
                task,
                start_date: Local::now(),
            };
            let infos_json = serde_json::to_string_pretty(&infos).into_diagnostic()?;
            std::fs::write(self.infos_file_path(), infos_json).into_diagnostic()?;

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
        #[cfg(not(unix))]
        {
            let _ = task;
            miette::bail!("The `start` command is only available on Unix systems.");
        }
    }

    /// Clear the run. This will delete the pid, stdout, stderr and infos files.
    pub fn clear(&self) -> miette::Result<()> {
        // check if the run is alive
        if self.is_alive() {
            miette::bail!("The run is still alive. You can call `pixi runs kill` to kill it.");
        }

        self.clear_force()
    }

    /// Clear the run even if it is alive. This will delete the pid, stdout, stderr and infos files.
    pub fn clear_force(&self) -> miette::Result<()> {
        // delete pid, infos, stdout and stderr files
        let _ = std::fs::remove_file(self.pid_file_path()).map_err(|_| {
            eprintln!(
                "{}Failed to remove pid file.",
                console::style(console::Emoji("⚠️ ", "")).yellow()
            );
        });

        let _ = std::fs::remove_file(self.infos_file_path()).map_err(|_| {
            eprintln!(
                "{}Failed to remove infos file.",
                console::style(console::Emoji("⚠️ ", "")).yellow()
            );
        });

        let _ = std::fs::remove_file(self.stdout_path()).map_err(|_| {
            eprintln!(
                "{}Failed to remove stdout file.",
                console::style(console::Emoji("⚠️ ", "")).yellow()
            );
        });

        let _ = std::fs::remove_file(self.stderr_path()).map_err(|_| {
            eprintln!(
                "{}Failed to remove stderr file.",
                console::style(console::Emoji("⚠️ ", "")).yellow()
            );
        });

        Ok(())
    }

    /// Get the state of the run. This will read the pid, stdout, stderr and infos files.
    pub fn state(&self) -> miette::Result<DaemonRunState> {
        let pid = match self.read_pid() {
            Some(pid) => pid,
            None => miette::bail!("Cannot read the pid file for the run '{}'.", self.name),
        };
        let stdout_length = self.read_stdout()?.len();
        let stderr_length = self.read_stderr()?.len();
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
            stdout_length,
            stderr_length,
        })
    }

    /// Kill the run. This will send a SIGTERM signal to the process.
    pub fn kill(&self) -> miette::Result<()> {
        let pid = match self.read_pid() {
            Some(pid) => pid,
            None => miette::bail!("Cannot read the pid file for the run '{}'.", self.name),
        };

        // `SystemInfo::refresh_system()` must have been call before.
        let system = SystemInfo::get();

        match system.process(pid) {
            // First try to terminate the process with a SIGTERM signal
            Some(process) => match process.kill_with(sysinfo::Signal::Term) {
                Some(result) => match result {
                    // All good if it works
                    true => Ok(()),
                    // If it doesn't work, try to kill the process with a SIGKILL signal
                    false => match process.kill_with(sysinfo::Signal::Kill) {
                        Some(result) => match result {
                            // All good if it works
                            true => Ok(()),
                            false => miette::bail!(
                                "Failed to terminate the process with pid '{}'.",
                                pid.as_u32()
                            ),
                        },
                        None => miette::bail!(
                            "Failed to terminate the process with pid '{}'.",
                            pid.as_u32()
                        ),
                    },
                },
                None => miette::bail!("The term signal does not exist on that platform"),
            },
            // if no process is associated with the pid, it means the process is terminated
            None => miette::bail!("No process with pid '{}' found.", pid.as_u32()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRunInfos {
    pub name: String,
    pub task: Vec<String>,
    pub start_date: DateTime<Local>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRunState {
    pub name: String,
    pub status: DaemonRunStatus,
    pub pid: u32,
    pub start_date: DateTime<Local>,
    pub task: Vec<String>,
    pub stdout_length: usize,
    pub stderr_length: usize,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DaemonRunStatus {
    Terminated,
    UnknownPid,
    // from https://docs.rs/sysinfo/latest/sysinfo/enum.ProcessStatus.html
    Idle,
    Run,
    Sleep,
    Stop,
    Zombie,
    Tracing,
    Dead,
    Wakekill,
    Waking,
    Parked,
    LockBlocked,
    UninterruptibleDiskSleep,
    Unknown(u32),
}

impl DaemonRunStatus {
    pub fn from_process_status(process_status: ProcessStatus) -> Self {
        match process_status {
            ProcessStatus::Idle => DaemonRunStatus::Idle,
            ProcessStatus::Run => DaemonRunStatus::Run,
            ProcessStatus::Sleep => DaemonRunStatus::Sleep,
            ProcessStatus::Stop => DaemonRunStatus::Stop,
            ProcessStatus::Zombie => DaemonRunStatus::Zombie,
            ProcessStatus::Tracing => DaemonRunStatus::Tracing,
            ProcessStatus::Dead => DaemonRunStatus::Dead,
            ProcessStatus::Wakekill => DaemonRunStatus::Wakekill,
            ProcessStatus::Waking => DaemonRunStatus::Waking,
            ProcessStatus::Parked => DaemonRunStatus::Parked,
            ProcessStatus::LockBlocked => DaemonRunStatus::LockBlocked,
            ProcessStatus::UninterruptibleDiskSleep => DaemonRunStatus::UninterruptibleDiskSleep,
            ProcessStatus::Unknown(u32) => DaemonRunStatus::Unknown(u32),
        }
    }
}

impl fmt::Display for DaemonRunStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

lazy_static::lazy_static! {
    /// Get the system info. This is cached.
    static ref SYSTEM: std::sync::Mutex<System> = std::sync::Mutex::new(System::new_all());
}

/// System info to help managing `sysinfo::System` as a singleton and also limiting the
/// number of `system.refresh_all()` calls.
pub struct SystemInfo {}

impl SystemInfo {
    /// Refresh the system info and return the system.
    pub fn refresh_and_get() -> std::sync::MutexGuard<'static, System> {
        let mut system = SYSTEM.lock().expect("Failed to lock system");
        system.refresh_all();
        system
    }

    /// Return the system.
    pub fn get() -> std::sync::MutexGuard<'static, System> {
        SYSTEM.lock().expect("Failed to lock system")
    }

    /// Refresh the system info.
    pub fn refresh() {
        let mut system = SYSTEM.lock().expect("Failed to lock system");
        system.refresh_all();
    }
}
