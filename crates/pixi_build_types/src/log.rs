//! Structured log records emitted by a build backend over stderr.
//!
//! When the frontend launches a backend it sets [`BACKEND_LOG_FORMAT_ENV`] to
//! [`BACKEND_LOG_FORMAT_JSON`]. The backend then serializes every `tracing`
//! event as a [`BackendLogRecord`] preceded by [`BACKEND_LOG_SENTINEL`] and
//! followed by a newline. Stderr lines without the sentinel are treated as
//! raw build output (e.g. compiler stdout/stderr forwarded by the backend).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Sentinel prefix that marks a stderr line as a structured [`BackendLogRecord`].
///
/// The two `U+001F` (Unit Separator) bytes around the tag make it
/// vanishingly unlikely to occur in normal build output while staying
/// printable enough to recognise when eyeballing a log.
pub const BACKEND_LOG_SENTINEL: &str = "\u{1f}pixi-log\u{1f}";

/// Environment variable read by the backend to pick a stderr log format.
pub const BACKEND_LOG_FORMAT_ENV: &str = "PIXI_BUILD_BACKEND_LOG_FORMAT";

/// Value of [`BACKEND_LOG_FORMAT_ENV`] that selects sentinel-tagged JSON.
pub const BACKEND_LOG_FORMAT_JSON: &str = "json";

/// A single structured log event emitted by the backend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendLogRecord {
    pub level: BackendLogLevel,
    pub target: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub fields: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<String>,
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
    fn record_round_trips_through_json() {
        let mut fields = Map::new();
        fields.insert("count".to_string(), Value::from(7));
        let record = BackendLogRecord {
            level: BackendLogLevel::Warn,
            target: "pixi_build::recipe".to_string(),
            message: "skipping unsupported variant".to_string(),
            fields,
            timestamp: Some("2026-06-09T12:00:00+00:00".to_string()),
            spans: vec!["build".into(), "render".into()],
        };
        let json = serde_json::to_string(&record).unwrap();
        let parsed: BackendLogRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.level, BackendLogLevel::Warn);
        assert_eq!(parsed.target, record.target);
        assert_eq!(parsed.message, record.message);
        assert_eq!(parsed.fields.get("count").and_then(|v| v.as_i64()), Some(7));
        assert_eq!(parsed.spans, record.spans);
    }

    #[test]
    fn empty_collections_omitted_in_wire_format() {
        let record = BackendLogRecord {
            level: BackendLogLevel::Info,
            target: "t".into(),
            message: "m".into(),
            fields: Map::new(),
            timestamp: None,
            spans: Vec::new(),
        };
        let json = serde_json::to_string(&record).unwrap();
        assert!(!json.contains("fields"));
        assert!(!json.contains("timestamp"));
        assert!(!json.contains("spans"));
    }
}
