use serde::Deserialize;
use serde_json::{Map, Value as JsonValue};
use tracing::Level;

const BACKEND_EVENT_TARGET: &str = "pixi::backend";

#[derive(Debug, Clone)]
pub struct BackendLogEntry {
    level: Level,
    target: String,
    message: String,
    extra: Map<String, JsonValue>,
}

impl BackendLogEntry {
    pub fn emit(&self) {
        let formatted_message = if self.extra.is_empty() {
            self.message.clone()
        } else {
            format!("{} {}", self.message, format_fields(&self.extra))
        };

        match self.level {
            Level::TRACE => {
                tracing::event!(target: BACKEND_EVENT_TARGET, Level::TRACE, backend_target = %self.target, "{formatted_message}")
            }
            Level::DEBUG => {
                tracing::event!(target: BACKEND_EVENT_TARGET, Level::DEBUG, backend_target = %self.target, "{formatted_message}")
            }
            Level::INFO => {
                tracing::event!(target: BACKEND_EVENT_TARGET, Level::INFO, backend_target = %self.target, "{formatted_message}")
            }
            Level::WARN => {
                tracing::event!(target: BACKEND_EVENT_TARGET, Level::WARN, backend_target = %self.target, "{formatted_message}")
            }
            Level::ERROR => {
                tracing::event!(target: BACKEND_EVENT_TARGET, Level::ERROR, backend_target = %self.target, "{formatted_message}")
            }
        }
    }
}

pub fn parse_backend_logs(line: &str) -> Option<Vec<BackendLogEntry>> {
    match serde_json::from_str::<TracingJsonLine>(line) {
        Ok(entry) => BackendLogEntry::from_tracing_json(entry).map(|entry| vec![entry]),
        Err(err) => {
            tracing::debug!(target: "pixi::backend_logs", "Failed to parse tracing JSON line: {err}");
            None
        }
    }
}

fn format_fields(fields: &Map<String, JsonValue>) -> String {
    fields
        .iter()
        .filter_map(|(key, value)| {
            if key == "message" {
                return None;
            }
            Some(format!("{}={}", key, value_to_string(value)))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn value_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(v) => maybe_quote(v),
        JsonValue::Number(num) => num.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(value_to_string).collect();
            format!("[{}]", parts.join(","))
        }
        JsonValue::Object(obj) => {
            let parts: Vec<String> = obj
                .iter()
                .map(|(k, v)| format!("{}={}", k, value_to_string(v)))
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        JsonValue::Null => "null".to_string(),
    }
}

fn maybe_quote(value: &str) -> String {
    if value.chars().any(|c| c.is_whitespace()) {
        format!("\"{}\"", value.replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

#[derive(Deserialize)]
struct TracingJsonLine {
    level: Option<String>,
    target: Option<String>,
    fields: Option<Map<String, JsonValue>>,
}

impl BackendLogEntry {
    fn from_tracing_json(line: TracingJsonLine) -> Option<Self> {
        let level = line
            .level
            .as_deref()
            .and_then(parse_level)
            .unwrap_or(Level::INFO);
        let target = line
            .target
            .unwrap_or_else(|| "pixi-build-backend".to_string());

        let mut fields = line.fields.unwrap_or_default();
        let message = fields
            .remove("message")
            .and_then(|value| value.as_str().map(|s| s.to_string()))
            .unwrap_or_default();

        Some(Self {
            level,
            target,
            message,
            extra: fields,
        })
    }
}

fn parse_level(value: &str) -> Option<Level> {
    match value.to_ascii_uppercase().as_str() {
        "TRACE" => Some(Level::TRACE),
        "DEBUG" => Some(Level::DEBUG),
        "INFO" => Some(Level::INFO),
        "WARN" | "WARNING" => Some(Level::WARN),
        "ERROR" => Some(Level::ERROR),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracing_json_line() {
        let sample = include_str!("../../tests/tracing_sample.json");
        let entries = parse_backend_logs(sample).expect("failed to parse tracing sample");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.level, Level::INFO);
        assert_eq!(entry.target, "pixi_build_backend::intermediate_backend");
        assert_eq!(entry.message, "sample message");
        assert!(entry.extra.contains_key("path"));
    }
}
