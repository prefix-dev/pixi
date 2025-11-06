use std::sync::Arc;

use crate::BackendOutputStream;
use serde_json::Value;
use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};
use tracing::Level;

/// Stderr stream that captures the stderr output of the backend and stores it
/// in a buffer for later use.
pub(crate) async fn stream_stderr<W: BackendOutputStream>(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    cancel: oneshot::Receiver<()>,
    mut on_log: W,
) -> Result<String, std::io::Error> {
    // Create a future that continuously read from the buffer and stores the lines
    // until all data is received.
    let mut lines = Vec::new();
    let read_and_buffer = async {
        let mut buffer = buffer.lock().await;
        while let Some(line) = buffer.next_line().await? {
            on_log.on_line(line.clone());
            lines.push(format_backend_line(&line));
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

fn format_backend_line(line: &str) -> String {
    parse_backend_log(line)
        .map(|log| log.format())
        .unwrap_or_else(|| line.to_owned())
}

fn parse_backend_log(line: &str) -> Option<ParsedBackendLog> {
    let value: Value = serde_json::from_str(line).ok()?;
    let level = match value.get("level")?.as_str()? {
        "TRACE" => Level::TRACE,
        "DEBUG" => Level::DEBUG,
        "INFO" => Level::INFO,
        "WARN" => Level::WARN,
        "ERROR" => Level::ERROR,
        _ => return None,
    };

    let target = value
        .get("target")
        .and_then(Value::as_str)
        .map(|s| s.to_owned());

    let message = value
        .get("fields")
        .and_then(|fields| {
            if let Some(msg) = fields.get("message").and_then(Value::as_str) {
                (!msg.is_empty()).then(|| msg.to_owned())
            } else if let Some(obj) = fields.as_object() {
                (!obj.is_empty()).then(|| fields.to_string())
            } else if let Some(text) = fields.as_str() {
                (!text.is_empty()).then(|| text.to_owned())
            } else {
                None
            }
        })
        .unwrap_or_else(|| line.to_owned());

    Some(ParsedBackendLog {
        level,
        target,
        message,
    })
}

struct ParsedBackendLog {
    level: Level,
    target: Option<String>,
    message: String,
}

impl ParsedBackendLog {
    fn format(&self) -> String {
        let level = self.level.as_str();
        match (&self.target, self.message.is_empty()) {
            (Some(target), false) => format!("[backend {level}] {target}: {}", self.message),
            (Some(target), true) => format!("[backend {level}] {target}"),
            (None, false) => format!("[backend {level}] {}", self.message),
            (None, true) => format!("[backend {level}]"),
        }
    }
}
