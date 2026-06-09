use std::sync::Arc;

use crate::BackendOutputStream;
use pixi_build_types::log::{BACKEND_LOG_SENTINEL, BackendLogLevel, BackendLogRecord};
use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};

/// Stderr stream that captures the stderr output of the backend and stores it
/// in a buffer for later use. Lines tagged with [`BACKEND_LOG_SENTINEL`] are
/// parsed as structured backend log records and re-emitted through the
/// frontend's `tracing` subscriber; other lines are forwarded to `on_log`
/// unchanged.
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
            match classify_line(&line) {
                ClassifiedLine::Record(record) => emit_record(&record),
                ClassifiedLine::Raw => {
                    on_log.on_line(line.clone());
                    lines.push(line);
                }
            }
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

enum ClassifiedLine {
    Record(BackendLogRecord),
    Raw,
}

fn classify_line(line: &str) -> ClassifiedLine {
    // Malformed sentinel lines fall through to `Raw` so the user still sees
    // them rather than having them silently dropped.
    line.strip_prefix(BACKEND_LOG_SENTINEL)
        .and_then(|json| serde_json::from_str::<BackendLogRecord>(json).ok())
        .map(ClassifiedLine::Record)
        .unwrap_or(ClassifiedLine::Raw)
}

/// Re-emit a [`BackendLogRecord`] received from a backend as a `tracing` event
/// on the frontend side. The event's target is rewritten to
/// `pixi_build_backend::<original-target>` so frontend filters can scope
/// backend logs independently.
fn emit_record(record: &BackendLogRecord) {
    // `tracing::event!` requires a const level, so dispatch per branch. Fields
    // are flattened into a JSON string because tracing has no runtime field
    // API — readers who care about structure can re-parse `fields`.
    let fields = if record.fields.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&record.fields).unwrap_or_default()
    };
    let spans = record.spans.join(">");
    let target = format!("pixi_build_backend::{}", record.target);

    macro_rules! emit {
        ($lvl:expr) => {{
            tracing::event!(
                target: "pixi_build_backend",
                $lvl,
                backend.target = %target,
                backend.spans = %spans,
                backend.fields = %fields,
                "{}",
                record.message,
            );
        }};
    }

    match record.level {
        BackendLogLevel::Trace => emit!(tracing::Level::TRACE),
        BackendLogLevel::Debug => emit!(tracing::Level::DEBUG),
        BackendLogLevel::Info => emit!(tracing::Level::INFO),
        BackendLogLevel::Warn => emit!(tracing::Level::WARN),
        BackendLogLevel::Error => emit!(tracing::Level::ERROR),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_line_without_sentinel_is_forwarded() {
        assert!(matches!(classify_line("cargo: compiling foo"), ClassifiedLine::Raw));
    }

    #[test]
    fn sentinel_line_with_valid_json_parses() {
        let payload = r#"{"level":"INFO","target":"pixi_build::recipe","message":"hello"}"#;
        let line = format!("{}{}", BACKEND_LOG_SENTINEL, payload);
        match classify_line(&line) {
            ClassifiedLine::Record(record) => {
                assert_eq!(record.level, BackendLogLevel::Info);
                assert_eq!(record.target, "pixi_build::recipe");
                assert_eq!(record.message, "hello");
            }
            ClassifiedLine::Raw => panic!("expected a structured record"),
        }
    }

    #[test]
    fn sentinel_line_with_malformed_json_falls_back_to_raw() {
        let line = format!("{}not json", BACKEND_LOG_SENTINEL);
        assert!(matches!(classify_line(&line), ClassifiedLine::Raw));
    }
}
