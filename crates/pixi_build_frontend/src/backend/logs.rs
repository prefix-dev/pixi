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
    match serde_json::from_str::<JsonValue>(line) {
        Ok(JsonValue::Object(map)) => {
            BackendLogEntry::from_tracing_fields(map).map(|entry| vec![entry])
        }
        Ok(_) => None,
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

impl BackendLogEntry {
    fn from_tracing_fields(mut map: Map<String, JsonValue>) -> Option<Self> {
        let level = map
            .remove("level")
            .and_then(|value| value.as_str().and_then(parse_level))
            .unwrap_or(Level::INFO);

        let target = map
            .remove("target")
            .and_then(|value| value.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "pixi-build-backend".to_string());

        let mut extra = Map::new();
        let mut message = take_message_from_fields(map.remove("fields"), &mut extra);

        if message.is_none() {
            if let Some(value) = map.remove("message") {
                message = Some(stringify_value(value));
            }
        } else {
            map.remove("message");
        }

        for (key, value) in map.into_iter() {
            if is_reserved_top_level_field(&key) {
                continue;
            }
            extra.insert(key, value);
        }

        Some(Self {
            level,
            target,
            message: message.unwrap_or_default(),
            extra,
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

fn take_message_from_fields(
    fields_value: Option<JsonValue>,
    extra_fields: &mut Map<String, JsonValue>,
) -> Option<String> {
    let mut message = None;
    if let Some(JsonValue::Object(mut fields)) = fields_value {
        message = take_message(&mut fields);
        for (key, value) in fields.into_iter() {
            extra_fields.insert(key, value);
        }
    }
    message
}

fn take_message(fields: &mut Map<String, JsonValue>) -> Option<String> {
    if let Some(value) = fields.remove("message") {
        return Some(stringify_value(value));
    }
    None
}

fn stringify_value(value: JsonValue) -> String {
    match value {
        JsonValue::String(s) => s,
        JsonValue::Null => "null".to_string(),
        other => value_to_string(&other),
    }
}

fn is_reserved_top_level_field(key: &str) -> bool {
    matches!(
        key,
        "timestamp"
            | "level"
            | "target"
            | "threadName"
            | "threadId"
            | "filename"
            | "line_number"
            | "span"
            | "spans"
    )
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

    #[test]
    fn parses_flattened_tracing_line() {
        let sample = include_str!("../../tests/tracing_flattened_sample.json");
        let entries = parse_backend_logs(sample).expect("failed to parse flattened sample");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.level, Level::DEBUG);
        assert_eq!(entry.target, "pixi_build_backend::flattened");
        assert_eq!(entry.message, "flattened message");
        assert_eq!(
            entry.extra.get("answer").and_then(|value| value.as_i64()),
            Some(42)
        );
        assert!(!entry.extra.contains_key("timestamp"));
        assert!(!entry.extra.contains_key("threadName"));
    }

    #[test]
    fn parses_span_metadata_line() {
        let sample = include_str!("../../tests/tracing_span_sample.json");
        let entries = parse_backend_logs(sample).expect("failed to parse span sample");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.level, Level::INFO);
        assert_eq!(entry.target, "pixi_build_backend::span");
        assert_eq!(entry.message, "span event");
        assert_eq!(
            entry.extra.get("busy_ns").and_then(|value| value.as_i64()),
            Some(123)
        );
    }
}
