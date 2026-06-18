//! Structured log records emitted by a build backend.
//!
//! When the frontend launches a backend it creates a listening Unix socket
//! (or Windows named pipe) and passes the address to the backend via
//! [`BACKEND_LOG_SOCKET_ENV`]. The backend connects to that address and
//! writes every `tracing` event and span lifecycle transition as a
//! [`BackendLogRecord`] JSON line. Build output (e.g. compiler stdout/stderr
//! forwarded by the backend) stays on stderr, so the two channels never
//! interleave.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Environment variable used by the frontend to tell the backend where to
/// connect for structured log delivery. The value is a Unix socket path on
/// Unix or a `\\.\pipe\...` named-pipe name on Windows. If unset the backend
/// keeps the old human-readable rendering on stderr.
pub const BACKEND_LOG_SOCKET_ENV: &str = "PIXI_BUILD_BACKEND_LOG_SOCKET";

/// A single record emitted by the backend on the log socket. Events are
/// emitted as they fire; span lifecycle records bracket their corresponding
/// events so the frontend can mirror the backend's span hierarchy with real
/// `tracing::Span`s rather than per-event fakes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BackendLogRecord {
    Event(BackendEventRecord),
    SpanOpen(BackendSpanOpenRecord),
    SpanClose(BackendSpanCloseRecord),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendEventRecord {
    pub level: BackendLogLevel,
    pub target: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Id of the span this event was emitted inside, if any. References a
    /// [`BackendSpanOpenRecord::id`] previously seen on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendSpanOpenRecord {
    pub id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<u64>,
    pub level: BackendLogLevel,
    pub target: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendSpanCloseRecord {
    pub id: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum BackendLogLevel {
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
    fn event_record_round_trips_through_json() {
        let mut fields = Map::new();
        fields.insert("count".to_string(), Value::from(7));
        let event = BackendEventRecord {
            level: BackendLogLevel::Warn,
            target: "pixi_build::recipe".to_string(),
            message: "skipping unsupported variant".to_string(),
            fields,
            timestamp: Some("2026-06-09T12:00:00+00:00".to_string()),
            span_id: Some(42),
        };
        let record = BackendLogRecord::Event(event);
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"kind\":\"event\""));
        let parsed: BackendLogRecord = serde_json::from_str(&json).unwrap();
        let BackendLogRecord::Event(parsed_event) = parsed else {
            panic!("expected Event variant");
        };
        assert_eq!(parsed_event.level, BackendLogLevel::Warn);
        assert_eq!(parsed_event.span_id, Some(42));
        assert_eq!(
            parsed_event.fields.get("count").and_then(|v| v.as_i64()),
            Some(7)
        );
    }

    #[test]
    fn span_open_round_trips() {
        let record = BackendLogRecord::SpanOpen(BackendSpanOpenRecord {
            id: 7,
            parent_id: Some(3),
            level: BackendLogLevel::Info,
            target: "pixi_build::build".to_string(),
            name: "render".to_string(),
            fields: Map::new(),
            timestamp: None,
        });
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"kind\":\"span_open\""));
    }

    #[test]
    fn span_close_round_trips() {
        let record = BackendLogRecord::SpanClose(BackendSpanCloseRecord { id: 11 });
        let json = serde_json::to_string(&record).unwrap();
        assert!(json.contains("\"kind\":\"span_close\""));
    }

    #[test]
    fn empty_collections_omitted_in_wire_format() {
        let record = BackendLogRecord::Event(BackendEventRecord {
            level: BackendLogLevel::Info,
            target: "t".into(),
            message: "m".into(),
            fields: Map::new(),
            timestamp: None,
            span_id: None,
        });
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("fields"));
        assert!(!json.contains("timestamp"));
        assert!(!json.contains("span_id"));
    }
}
