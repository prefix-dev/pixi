//! Structured log records emitted by a build backend as JSON-RPC
//! *notifications* on the same stdio channel that carries requests and
//! responses.
//!
//! The backend must only send these notifications if the frontend advertised
//! [`crate::capabilities::FrontendCapabilities::supports_log_messages`]
//! during capability negotiation. Frontends that do not understand the
//! method simply never advertise it, and backends that do not implement it
//! never send it, so the feature degrades cleanly in both directions
//! without a Pixi Build API version bump.
//!
//! Two kinds of information travel over this channel:
//!
//! * [`LogMessage::Event`]: a single `tracing` event (log line) emitted by
//!   the backend. INFO-level events are deliberately *not* sent: by
//!   convention (inherited from rattler-build) they form the plaintext
//!   build-output stream and keep flowing over stderr where the frontend
//!   captures them for progress display and failure replay.
//! * [`LogMessage::SpanOpen`] / [`LogMessage::SpanClose`]: the lifecycle of
//!   the backend's `tracing` spans, so the frontend can mirror the span
//!   hierarchy with real spans and events render with their full
//!   `outer:inner:` context.
//!
//! Span ids are the backend's own `tracing` span ids. They are unique for
//! the lifetime of the backend process, which is safe here because the
//! notification channel lives exactly as long as the backend process.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const METHOD_NAME: &str = "log/message";

/// Environment variable the frontend sets on the backend process to raise
/// the backend's log verbosity to its own. Holds a level name (`error`,
/// `warn`, `info`, `debug` or `trace`). The backend treats it as a *floor*:
/// the effective verbosity is the maximum of this value and the backend's
/// own command line flags.
pub const LOG_LEVEL_ENV: &str = "PIXI_BUILD_BACKEND_LOG_LEVEL";

/// A single record emitted by the backend on the log channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogMessage {
    /// A `tracing` event.
    Event(LogEvent),
    /// A `tracing` span was opened.
    SpanOpen(LogSpanOpen),
    /// A previously opened span was closed.
    SpanClose(LogSpanClose),
}

/// A single `tracing` event emitted by the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEvent {
    /// The level of the event.
    pub level: LogLevel,
    /// The `tracing` target of the event (usually the module path).
    pub target: String,
    /// The formatted message of the event.
    pub message: String,
    /// Additional structured fields recorded on the event.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    /// Id of the span this event was emitted inside, if any. References a
    /// [`LogSpanOpen::id`] previously seen on the channel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<u64>,
}

/// A span that was opened in the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSpanOpen {
    /// The backend-process-unique id of the span.
    pub id: u64,
    /// The id of the parent span, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
    /// The level of the span.
    pub level: LogLevel,
    /// The `tracing` target of the span.
    pub target: String,
    /// The name of the span.
    pub name: String,
    /// The fields recorded on the span.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
}

/// A span that was closed in the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogSpanClose {
    /// The id of the span that was closed.
    pub id: u64,
}

/// The severity of a log record, mirroring `tracing::Level`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_round_trips_through_json() {
        let mut fields = Map::new();
        fields.insert("count".to_string(), Value::from(7));
        let record = LogMessage::Event(LogEvent {
            level: LogLevel::Warn,
            target: "pixi_build::recipe".to_string(),
            message: "skipping unsupported variant".to_string(),
            fields,
            span_id: Some(42),
        });
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"kind\":\"event\""));
        let LogMessage::Event(parsed) = serde_json::from_str(&json).unwrap() else {
            panic!("expected Event variant");
        };
        assert_eq!(parsed.level, LogLevel::Warn);
        assert_eq!(parsed.span_id, Some(42));
        assert_eq!(parsed.fields.get("count").and_then(Value::as_i64), Some(7));
    }

    #[test]
    fn span_lifecycle_round_trips_through_json() {
        let open = LogMessage::SpanOpen(LogSpanOpen {
            id: 7,
            parent_id: Some(3),
            level: LogLevel::Info,
            target: "pixi_build::build".to_string(),
            name: "render".to_string(),
            fields: Map::new(),
        });
        let json = serde_json::to_string(&open).unwrap();
        assert!(json.contains("\"kind\":\"span_open\""));
        let LogMessage::SpanOpen(parsed) = serde_json::from_str(&json).unwrap() else {
            panic!("expected SpanOpen variant");
        };
        assert_eq!(parsed.parent_id, Some(3));

        let close = LogMessage::SpanClose(LogSpanClose { id: 7 });
        let json = serde_json::to_string(&close).unwrap();
        assert!(json.contains("\"kind\":\"span_close\""));
    }

    #[test]
    fn empty_collections_are_omitted_from_the_wire_format() {
        let record = LogMessage::Event(LogEvent {
            level: LogLevel::Info,
            target: "t".into(),
            message: "m".into(),
            fields: Map::new(),
            span_id: None,
        });
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("fields"));
        assert!(!json.contains("span_id"));
    }
}
