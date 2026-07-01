//! Backend side of the structured log channel.
//!
//! When the frontend sets [`BACKEND_LOG_SOCKET_ENV`], the backend connects to
//! the named address (Unix socket path or Windows named-pipe name) at
//! startup and spawns an async writer task. [`JsonLogLayer`] pushes each
//! serialised record into an unbounded mpsc channel; the writer task drains
//! the channel and writes records to the connected stream as one JSON line
//! per record. Backpressure isn't a concern in practice (log volume is
//! modest), and any I/O error tears down the channel silently so the
//! backend keeps making progress.
//!
//! [`JsonLogLayer`]: crate::json_log_layer::JsonLogLayer
//! [`BACKEND_LOG_SOCKET_ENV`]: pixi_build_types::log::BACKEND_LOG_SOCKET_ENV

use std::pin::Pin;

use pixi_build_types::log::BackendLogRecord;
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

/// Send half held by the tracing layer. Cloning is cheap.
#[derive(Clone)]
pub struct LogChannelSender {
    inner: mpsc::UnboundedSender<BackendLogRecord>,
}

impl LogChannelSender {
    pub fn send(&self, record: BackendLogRecord) {
        // Drop the record if the receiver is gone — the writer task has shut
        // down and there's nowhere useful to send.
        let _ = self.inner.send(record);
    }
}

/// Connect to the address the frontend handed us and spawn an async writer
/// task that drains [`LogChannelSender`] to the wire. The returned sender is
/// what `JsonLogLayer` writes through.
///
/// On any I/O error during connect this returns `None` and the caller should
/// fall back to whatever default log routing was configured.
pub async fn connect_and_spawn(addr: &str) -> Option<LogChannelSender> {
    let stream: Pin<Box<dyn AsyncWrite + Send>> = match open_stream(addr).await {
        Ok(s) => s,
        Err(_) => return None,
    };

    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(write_loop(stream, rx));
    Some(LogChannelSender { inner: tx })
}

async fn write_loop(
    mut stream: Pin<Box<dyn AsyncWrite + Send>>,
    mut rx: mpsc::UnboundedReceiver<BackendLogRecord>,
) {
    let mut line = Vec::with_capacity(512);
    while let Some(record) = rx.recv().await {
        line.clear();
        if serde_json::to_writer(&mut line, &record).is_err() {
            continue;
        }
        line.push(b'\n');
        if stream.write_all(&line).await.is_err() {
            // Frontend hung up — drain remaining records and bail.
            break;
        }
    }
    let _ = stream.shutdown().await;
}

#[cfg(unix)]
async fn open_stream(
    path: &str,
) -> std::io::Result<Pin<Box<dyn AsyncWrite + Send>>> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    // We only ever write, so split off and discard the read half.
    let (_read, write) = stream.into_split();
    Ok(Box::pin(write))
}

#[cfg(windows)]
async fn open_stream(
    pipe_name: &str,
) -> std::io::Result<Pin<Box<dyn AsyncWrite + Send>>> {
    use tokio::net::windows::named_pipe::ClientOptions;
    // The frontend has already created the server end; a single open() here
    // produces the connected client. If the server isn't ready yet we get
    // ERROR_PIPE_BUSY — retry a few times with short backoff.
    let mut last_err = None;
    for _ in 0..10 {
        match ClientOptions::new().open(pipe_name) {
            Ok(client) => return Ok(Box::pin(client)),
            Err(e) => {
                last_err = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::TimedOut, "named pipe not ready")
    }))
}
