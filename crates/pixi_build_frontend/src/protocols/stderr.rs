use std::sync::Arc;

use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::{mpsc, oneshot, Mutex},
};

/// Stderr sink that captures the stderr output of the backend
/// but does not do anything with it.
pub async fn stderr_null(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    cancel: oneshot::Receiver<()>,
) -> Result<(), std::io::Error> {
    tokio::select! {
        // Please stop
        _ = cancel => {
            Ok(())
        }
        // Please keep reading
        result = async {
            let mut lines = buffer.lock().await;
            while let Some(_line) = lines.next_line().await? {}
            Ok(())
        } => {
            result
        }
    }
}

/// Stderr stream that captures the stderr output of the backend
/// and sends it over the stream.
pub async fn stderr_stream(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    sender: mpsc::Sender<String>,
    cancel: oneshot::Receiver<()>,
) -> Result<(), std::io::Error> {
    tokio::select! {
        _ = cancel => {
            Ok(())
        }
        result = async {
            let mut lines = buffer.lock().await;
            while let Some(line) = lines.next_line().await? {
                if let Err(err) = sender.send(line).await {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, err));
                }
            }
            Ok(())
        } => {
            result
        }
    }
}
