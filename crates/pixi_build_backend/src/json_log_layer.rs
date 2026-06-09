//! Tracing layer that emits [`BackendLogRecord`] JSON lines on stderr.
//!
//! Each event is written as `{sentinel}{json}\n`, where `sentinel` is
//! [`BACKEND_LOG_SENTINEL`]. The pixi frontend reads stderr line by line and
//! re-emits records prefixed with the sentinel through its own `tracing`
//! subscriber, so the user sees backend logs interleaved with frontend logs.

use std::io::Write;

use chrono::Utc;
use pixi_build_types::log::{BACKEND_LOG_SENTINEL, BackendLogLevel, BackendLogRecord};
use serde_json::{Map, Value};
use tracing::{Event, Level, Subscriber, field::Visit};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

pub struct JsonLogLayer;

struct FieldVisitor {
    fields: Map<String, Value>,
    message: Option<String>,
}

impl FieldVisitor {
    fn new() -> Self {
        Self {
            fields: Map::new(),
            message: None,
        }
    }

    fn insert(&mut self, name: &str, value: Value) {
        if name == "message" {
            // Tracing's `message` field is always serialized through the
            // dedicated `message` slot in the record, not in `fields`.
            if let Value::String(s) = value {
                self.message = Some(s);
            } else {
                self.message = Some(value.to_string());
            }
        } else {
            self.fields.insert(name.to_string(), value);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.insert(field.name(), Value::String(value.to_owned()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.insert(field.name(), Value::Bool(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.insert(field.name(), Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.insert(field.name(), Value::Number(value.into()));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        match serde_json::Number::from_f64(value) {
            Some(n) => self.insert(field.name(), Value::Number(n)),
            None => self.insert(field.name(), Value::String(value.to_string())),
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.insert(field.name(), Value::String(format!("{:?}", value)));
    }
}

fn level_to_backend(level: &Level) -> BackendLogLevel {
    match *level {
        Level::TRACE => BackendLogLevel::Trace,
        Level::DEBUG => BackendLogLevel::Debug,
        Level::INFO => BackendLogLevel::Info,
        Level::WARN => BackendLogLevel::Warn,
        Level::ERROR => BackendLogLevel::Error,
    }
}

impl<S> Layer<S> for JsonLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::new();
        event.record(&mut visitor);

        let metadata = event.metadata();
        let spans = ctx
            .event_scope(event)
            .map(|scope| {
                scope
                    .from_root()
                    .map(|span| span.name().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let record = BackendLogRecord {
            level: level_to_backend(metadata.level()),
            target: metadata.target().to_string(),
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
            timestamp: Some(Utc::now().to_rfc3339()),
            spans,
        };

        let Ok(json) = serde_json::to_string(&record) else {
            return;
        };

        // A single `write_all` (one syscall) keeps the sentinel+payload+newline
        // atomic against interleaving with raw stderr writes from sibling
        // tasks. Failures are silently dropped — there's no reasonable
        // recourse from inside a tracing layer.
        let mut line = String::with_capacity(BACKEND_LOG_SENTINEL.len() + json.len() + 1);
        line.push_str(BACKEND_LOG_SENTINEL);
        line.push_str(&json);
        line.push('\n');
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }
}
