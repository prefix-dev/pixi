use std::sync::Arc;

use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::{mpsc, oneshot, Mutex},
};

/// Stderr stream that captures the stderr output of the backend
/// and sends it over the stream.
pub(crate) async fn stderr_stream(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    sender: mpsc::Sender<String>,
    cancel: oneshot::Receiver<()>,
) -> Result<(), std::io::Error> {
    // Create a future that continuously read from the buffer and sends individual
    // lines over a channel.
    let read_and_forward = async {
        let mut lines = buffer.lock().await;
        while let Some(line) = lines.next_line().await? {
            if let Err(err) = sender.send(line).await {
                return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
            }
        }
        Ok(())
    };

    // Either await until the cancel signal is received or the `read_and_forward`
    // future is done, meaning there is not more data to read.
    tokio::select! {
        _ = cancel => {
            Ok(())
        }
        result = read_and_forward => {
            result
        }
    }
}

/// Stderr stream that captures the stderr output of the backend and stores it
/// in a buffer for later use.
pub(crate) async fn stderr_buffer(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    cancel: oneshot::Receiver<()>,
) -> Result<String, std::io::Error> {
    // Create a future that continuously read from the buffer and stores the lines
    // until all data is received.
    let mut lines = Vec::new();
    let read_and_buffer = async {
        let mut buffer = buffer.lock().await;
        while let Some(line) = buffer.next_line().await? {
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
