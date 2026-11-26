//! Tasks that are started in the background by `depends-on` entries marked
//! with `background = true`.

use std::{collections::HashMap, ffi::OsString, future::Future, rc::Rc, time::Duration};

use deno_task_shell::{
    KillSignal, ShellCommand, ShellPipeWriter, ShellState, SignalKind, execute_with_pipes, pipe,
};
use pixi_task::ExecutableTask;
use tokio::{
    task::{JoinHandle, LocalSet},
    time::timeout,
};

use super::RunError;

/// How long background tasks get to exit after being asked to stop before they
/// are killed.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

struct BackgroundTask {
    name: String,
    kill_signal: KillSignal,
    handle: JoinHandle<i32>,
}

/// The background tasks of a single `pixi run`.
///
/// The tasks live on their own [`LocalSet`] that is only polled while pixi
/// waits on [`BackgroundTasks::run_until`] or [`BackgroundTasks::stop`].
#[derive(Default)]
pub(super) struct BackgroundTasks {
    local_set: LocalSet,
    tasks: Vec<BackgroundTask>,
}

impl BackgroundTasks {
    /// Starts the task without waiting for it. It has a closed stdin and shares
    /// stdout and stderr with pixi.
    pub async fn spawn(
        &mut self,
        task: &ExecutableTask<'_, '_>,
        command_env: &HashMap<OsString, OsString>,
        kill_signal: &KillSignal,
        custom_commands: &HashMap<String, Rc<dyn ShellCommand>>,
    ) -> Result<(), RunError> {
        let Some(script) = task.as_deno_script()? else {
            return Ok(());
        };
        let cwd = task.working_directory()?;

        // Signals sent to the task do not reach the foreground tasks, but
        // signals sent to the run reach the task.
        let kill_signal = kill_signal.child_signal();
        let state = ShellState::new(
            command_env.clone(),
            cwd,
            custom_commands.clone(),
            kill_signal.clone(),
        );
        let (stdin, _closed_writer) = pipe();
        let handle = self.local_set.spawn_local(execute_with_pipes(
            script,
            state,
            stdin,
            ShellPipeWriter::stdout(),
            ShellPipeWriter::stderr(),
        ));
        self.tasks.push(BackgroundTask {
            name: task.name().unwrap_or("unnamed").to_string(),
            kill_signal,
            handle,
        });

        // Give the task the chance to start before the next one does.
        self.local_set.run_until(tokio::task::yield_now()).await;
        Ok(())
    }

    /// Runs the future while driving the background tasks.
    pub async fn run_until<F: Future>(&self, future: F) -> F::Output {
        self.local_set.run_until(future).await
    }

    /// Stops all background tasks and waits until they have exited.
    ///
    /// The tasks are asked to terminate first and are killed if they are still
    /// running after [`STOP_TIMEOUT`]. They have to be awaited: dropping them
    /// would leave their processes running.
    pub async fn stop(&mut self) {
        if self.tasks.is_empty() {
            return;
        }

        for task in &self.tasks {
            if task.kill_signal.aborted_code().is_none() {
                task.kill_signal.send(SignalKind::SIGTERM);
            }
        }

        if timeout(STOP_TIMEOUT, &mut self.local_set).await.is_ok() {
            return;
        }

        for task in self.tasks.iter().filter(|task| !task.handle.is_finished()) {
            tracing::warn!(
                "background task '{}' did not stop within {} seconds, killing it",
                task.name,
                STOP_TIMEOUT.as_secs()
            );
            task.kill_signal.send(SignalKind::SIGKILL);
        }
        (&mut self.local_set).await;
    }
}
