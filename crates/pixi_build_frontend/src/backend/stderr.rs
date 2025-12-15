use std::{fs::OpenOptions, io::Write, sync::Arc};

use crate::{BackendOutputStream, backend::logs::parse_backend_logs};
use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};

/// Stderr stream that captures the stderr output of the backend and stores it
/// in a buffer for later use.
pub(crate) async fn stream_stderr<W: BackendOutputStream>(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    cancel: oneshot::Receiver<()>,
    mut on_log: W,
) -> Result<String, std::io::Error> {
    let dump_raw_path = std::env::var_os("PIXI_DUMP_BACKEND_RAW");

    // Create a future that continuously read from the buffer and stores the lines
    // until all data is received.
    let mut lines = Vec::new();
    let read_and_buffer = async {
        let mut buffer = buffer.lock().await;
        while let Some(line) = buffer.next_line().await? {
            if let Some(path) = dump_raw_path.as_ref() {
                let _ = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .and_then(|mut file| writeln!(file, "{line}"));
            }

            if let Some(entries) = parse_backend_logs(&line) {
                for entry in entries {
                    entry.emit();
                }
                on_log.on_line(line.clone());
                lines.push(line);
                continue;
            }

            on_log.on_line(line.clone());
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
