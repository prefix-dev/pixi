//! Process-scoped pump for the backend's stderr stream.
//!
//! Structured logs travel over the dedicated log socket (see
//! [`crate::backend::log_channel`]). This pump exists solely to drain the
//! backend process's stderr — which only carries raw build output now
//! (compiler messages, INFO-level rattler-build progress, etc.) — and to
//! fan it out to per-RPC subscribers.

use std::sync::{Arc, Mutex as StdMutex};

use crate::BackendOutputStream;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::ChildStderr,
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};

pub(crate) struct StderrPump {
    /// Active subscribers receiving raw lines. Dead senders are removed
    /// lazily by the pump loop, so callers don't need to deregister.
    subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>>,
    /// Handle to the pump task. Aborted on drop so an orphaned backend
    /// doesn't leave the pump spinning.
    task: StdMutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for StderrPump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StderrPump").finish_non_exhaustive()
    }
}

impl StderrPump {
    pub(crate) fn spawn(stderr: ChildStderr) -> Arc<Self> {
        let subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let subscribers_for_task = subscribers.clone();
        let task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let mut subs = subscribers_for_task.lock().await;
                if subs.is_empty() {
                    continue;
                }
                subs.retain(|sender| sender.send(line.clone()).is_ok());
            }
        });
        Arc::new(Self {
            subscribers,
            task: StdMutex::new(Some(task)),
        })
    }

    /// Subscribe to raw lines for the duration of `cancel`, forwarding each
    /// to `sink` and accumulating them into the returned string for error
    /// reporting.
    pub(crate) async fn run_with_sink<W>(
        self: Arc<Self>,
        mut sink: W,
        cancel: oneshot::Receiver<()>,
    ) -> Result<String, std::io::Error>
    where
        W: BackendOutputStream + Send + 'static,
    {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        self.subscribers.lock().await.push(sender);

        let mut lines = Vec::new();
        let read_loop = async {
            while let Some(line) = receiver.recv().await {
                sink.on_line(line.clone());
                lines.push(line);
            }
            Ok::<String, std::io::Error>(lines.join("\n"))
        };

        let result = tokio::select! {
            _ = cancel => Ok(lines.join("\n")),
            r = read_loop => r,
        };
        drop(receiver);
        result
    }
}

impl Drop for StderrPump {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.task.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}
