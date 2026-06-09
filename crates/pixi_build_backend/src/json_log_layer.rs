//! Tracing layer that emits [`BackendLogRecord`] JSON lines on stderr.
//!
//! Each event and span lifecycle transition is written as
//! `{sentinel}{json}\n`, where `sentinel` is [`BACKEND_LOG_SENTINEL`]. The
//! pixi frontend reads stderr line by line, parses records, and replays span
//! lifetimes through its own `tracing` subscriber so backend logs nest the
//! same way they did inside the backend.

use std::io::Write;

use chrono::Utc;
use pixi_build_types::log::{
    BACKEND_LOG_SENTINEL, BackendEventRecord, BackendLogLevel, BackendLogRecord,
    BackendSpanCloseRecord, BackendSpanOpenRecord,
};
use serde_json::{Map, Value};
use tracing::{
    Event, Level, Subscriber,
    field::Visit,
    span::{Attributes, Id},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

pub struct JsonLogLayer;

struct FieldVisitor {
    fields: Map<String, Value>,
    message: Option<String>,
    extract_message: bool,
}

impl FieldVisitor {
    fn new_event() -> Self {
        Self {
            fields: Map::new(),
            message: None,
            extract_message: true,
        }
    }

    fn new_span() -> Self {
        Self {
            fields: Map::new(),
            message: None,
            extract_message: false,
        }
    }

    fn insert(&mut self, name: &str, value: Value) {
        if self.extract_message && name == "message" {
            // Tracing's `message` field is always serialised through the
            // dedicated `message` slot on event records, not in `fields`.
            self.message = Some(match value {
                Value::String(s) => s,
                other => other.to_string(),
            });
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

fn now_rfc3339() -> Option<String> {
    Some(Utc::now().to_rfc3339())
}

/// Write a single record as `{sentinel}{json}\n` to stderr. The whole line
/// goes through one `write_all` call so the prefix and payload stay together
/// — but note that on a pipe this is only guaranteed atomic up to
/// `PIPE_BUF` (4 KiB on Linux). Larger payloads may interleave with concurrent
/// stderr writers; rare in practice but worth knowing.
fn emit(record: &BackendLogRecord) {
    let Ok(json) = serde_json::to_string(record) else {
        return;
    };
    let mut line = String::with_capacity(BACKEND_LOG_SENTINEL.len() + json.len() + 1);
    line.push_str(BACKEND_LOG_SENTINEL);
    line.push_str(&json);
    line.push('\n');
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

impl<S> Layer<S> for JsonLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::new_span();
        attrs.record(&mut visitor);
        let metadata = attrs.metadata();
        // Prefer the explicit parent set via `parent:` on the macro; fall back
        // to the contextually-current span (the default in tracing).
        let parent_id = attrs
            .parent()
            .cloned()
            .or_else(|| ctx.current_span().id().cloned())
            .map(|i| i.into_u64());

        emit(&BackendLogRecord::SpanOpen(BackendSpanOpenRecord {
            id: id.into_u64(),
            parent_id,
            level: level_to_backend(metadata.level()),
            target: metadata.target().to_string(),
            name: metadata.name().to_string(),
            fields: visitor.fields,
            timestamp: now_rfc3339(),
        }));
    }

    fn on_close(&self, id: Id, _ctx: Context<'_, S>) {
        emit(&BackendLogRecord::SpanClose(BackendSpanCloseRecord {
            id: id.into_u64(),
        }));
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::new_event();
        event.record(&mut visitor);
        let metadata = event.metadata();
        let span_id = ctx.event_span(event).map(|s| s.id().into_u64());

        emit(&BackendLogRecord::Event(BackendEventRecord {
            level: level_to_backend(metadata.level()),
            target: metadata.target().to_string(),
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
            timestamp: now_rfc3339(),
            span_id,
        }));
    }
}
