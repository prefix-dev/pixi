use std::sync::Arc;

use jsonrpsee::core::client::Subscription;
use pixi_build_types::procedures::log::LogParams;
use tokio::sync::{Mutex, oneshot};

use crate::{
    BackendOutputStream,
    backend::{json_rpc::BackendStderr, render_stderr_line},
};

/// Stderr stream that captures the stderr output of the backend and stores it
/// in a buffer for later use.
pub(crate) async fn stream_stderr<W: BackendOutputStream>(
    buffer: Arc<Mutex<BackendStderr>>,
    cancel: oneshot::Receiver<()>,
    on_log: Arc<Mutex<W>>,
) -> Result<String, std::io::Error> {
    // Create a future that continuously read from the buffer and stores the lines
    // until all data is received.
    let mut lines = Vec::new();
    let read_and_buffer = async {
        let mut buffer = buffer.lock().await;
        while let Some(line) = buffer.next_line().await? {
            // The stream gets the labelled line; the buffer keeps the raw one,
            // because it is quoted verbatim when reporting a premature exit.
            on_log.lock().await.on_line(render_stderr_line(&line));
            lines.push(line);
        }
        Ok(lines.join("\n"))
    };

    // Either wait until the cancel signal is received or the `read_and_buffer`
    // finishes which means there is no more data to read.
    tokio::select! {
        _ = cancel => {
            Ok(lines.join("\n"))
        }
        result = read_and_buffer => {
            result
        }
    }
}

/// Forwards the structured log events the backend sends over the connection to
/// the output stream, until cancelled.
///
/// Runs alongside [`stream_stderr`]: a backend that forwards its log events
/// still writes unstructured output -- anything a subprocess it spawned prints,
/// and its own panics -- to stderr.
pub(crate) async fn stream_logs<W: BackendOutputStream>(
    subscription: Arc<Mutex<Subscription<LogParams>>>,
    cancel: oneshot::Receiver<()>,
    on_log: Arc<Mutex<W>>,
) {
    let forward = async {
        let mut subscription = subscription.lock().await;
        while let Some(log) = subscription.next().await {
            match log {
                Ok(log) => on_log.lock().await.on_log(log),
                // A malformed notification says nothing about the build itself,
                // and the build's own errors are reported through the response.
                Err(err) => tracing::debug!("ignoring an invalid log notification: {err}"),
            }
        }
    };

    tokio::select! {
        _ = cancel => {}
        () = forward => {}
    }
}
