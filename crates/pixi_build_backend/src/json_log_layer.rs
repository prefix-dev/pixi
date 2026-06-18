//! Tracing layer that ships [`BackendLogRecord`]s over the structured log
//! channel established at backend startup by [`crate::log_channel`].
//!
//! The layer captures every event and span lifecycle transition, converts
//! them into [`BackendLogRecord`]s, and hands them off to a
//! [`LogChannelSender`]. The actual socket I/O happens in a separate async
//! writer task so synchronous `tracing` callbacks never block.
//!
//! INFO events are skipped on purpose: rattler-build uses them as its
//! plaintext build-output channel and the frontend renders those directly
//! through [`rattler_build_core::console_utils::LoggingOutputHandler`].
//! Span lifecycle is emitted for all levels so non-INFO events under an
//! INFO span keep their parent context on the frontend.

use chrono::Utc;
use pixi_build_types::log::{
    BackendEventRecord, BackendLogLevel, BackendLogRecord, BackendSpanCloseRecord,
    BackendSpanOpenRecord,
};
use serde_json::{Map, Value};
use tracing::{
    Event, Level, Subscriber,
    field::Visit,
    span::{Attributes, Id},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use crate::log_channel::LogChannelSender;

pub struct JsonLogLayer {
    sender: LogChannelSender,
}

impl JsonLogLayer {
    pub fn new(sender: LogChannelSender) -> Self {
        Self { sender }
    }
}

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

impl<S> Layer<S> for JsonLogLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::new_span();
        attrs.record(&mut visitor);
        let metadata = attrs.metadata();
        let parent_id = attrs
            .parent()
            .cloned()
            .or_else(|| ctx.current_span().id().cloned())
            .map(|i| i.into_u64());

        self.sender.send(BackendLogRecord::SpanOpen(BackendSpanOpenRecord {
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
        self.sender.send(BackendLogRecord::SpanClose(BackendSpanCloseRecord {
            id: id.into_u64(),
        }));
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if *metadata.level() == Level::INFO {
            return;
        }

        let mut visitor = FieldVisitor::new_event();
        event.record(&mut visitor);
        let span_id = ctx.event_span(event).map(|s| s.id().into_u64());

        self.sender.send(BackendLogRecord::Event(BackendEventRecord {
            level: level_to_backend(metadata.level()),
            target: metadata.target().to_string(),
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
            timestamp: now_rfc3339(),
            span_id,
        }));
    }
}
