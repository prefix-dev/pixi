//! A `tracing` layer that converts events and span lifecycles into
//! [`LogMessage`] records which the server forwards to the frontend as
//! `log/message` JSON-RPC notifications (see
//! [`crate::server::Server::with_log_messages`]).
//!
//! INFO-level events are skipped on purpose: by convention (inherited from
//! rattler-build) they form the plaintext build-output stream and are
//! rendered to stderr by `LoggingOutputHandler`, where the frontend captures
//! them for live display and failure replay.
//!
//! Span-open records are emitted *lazily*: when a span is created its record
//! is stashed in the span's extensions, and only when an event actually
//! flows through the channel are the unsent records of its ancestor chain
//! flushed first. This keeps spans that never produce a forwarded event off
//! the wire, and it transparently handles spans created before the frontend
//! has negotiated `supports_log_messages` (their records are still pending
//! and get flushed with the first event after activation).

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use pixi_build_types::procedures::log_message::{
    LogEvent, LogLevel, LogMessage, LogSpanClose, LogSpanOpen,
};
use serde_json::{Map, Value};
use tokio::sync::mpsc;
use tracing::{
    Event, Level, Subscriber,
    field::Visit,
    span::{Attributes, Id},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// State of a span's open record, stored in the span's extensions.
enum SpanOpenState {
    /// The record has not been sent yet.
    Pending(LogSpanOpen),
    /// The record was sent; a close record must follow on span close.
    Sent,
}

pub struct LogMessageLayer {
    sender: mpsc::UnboundedSender<LogMessage>,
    /// Flipped by the server once the frontend advertises
    /// `supports_log_messages` during capability negotiation.
    enabled: Arc<AtomicBool>,
}

impl LogMessageLayer {
    pub fn new(sender: mpsc::UnboundedSender<LogMessage>, enabled: Arc<AtomicBool>) -> Self {
        Self { sender, enabled }
    }

    fn send(&self, record: LogMessage) {
        // If the receiver is gone the server has shut down; there is nowhere
        // useful to send the record to.
        let _ = self.sender.send(record);
    }
}

impl<S> Layer<S> for LogMessageLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::for_span();
        attrs.record(&mut visitor);
        let metadata = attrs.metadata();
        let parent_id = attrs
            .parent()
            .cloned()
            .or_else(|| ctx.current_span().id().cloned())
            .map(|id| id.into_u64());

        let record = LogSpanOpen {
            id: id.into_u64(),
            parent_id,
            level: level_to_wire(metadata.level()),
            target: metadata.target().to_string(),
            name: metadata.name().to_string(),
            fields: visitor.fields,
        };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(SpanOpenState::Pending(record));
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        // Only spans whose open record went over the wire need a close.
        if matches!(span.extensions().get(), Some(SpanOpenState::Sent)) {
            self.send(LogMessage::SpanClose(LogSpanClose { id: id.into_u64() }));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if !self.enabled.load(Ordering::Acquire) {
            return;
        }
        let metadata = event.metadata();
        if *metadata.level() == Level::INFO {
            return;
        }

        // Flush unsent span-open records root-first so the frontend always
        // sees a parent before anything that references it.
        let span_id = ctx.event_span(event).map(|leaf| {
            for span in leaf.scope().from_root() {
                let mut extensions = span.extensions_mut();
                if let Some(state) = extensions.get_mut::<SpanOpenState>()
                    && let SpanOpenState::Pending(record) =
                        std::mem::replace(state, SpanOpenState::Sent)
                {
                    self.send(LogMessage::SpanOpen(record));
                }
            }
            leaf.id().into_u64()
        });

        let mut visitor = FieldVisitor::for_event();
        event.record(&mut visitor);
        self.send(LogMessage::Event(LogEvent {
            level: level_to_wire(metadata.level()),
            target: metadata.target().to_string(),
            message: visitor.message.unwrap_or_default(),
            fields: visitor.fields,
            span_id,
        }));
    }
}

fn level_to_wire(level: &Level) -> LogLevel {
    match *level {
        Level::TRACE => LogLevel::Trace,
        Level::DEBUG => LogLevel::Debug,
        Level::INFO => LogLevel::Info,
        Level::WARN => LogLevel::Warn,
        Level::ERROR => LogLevel::Error,
    }
}

/// Collects the fields of an event or span into a JSON map, optionally
/// extracting the conventional `message` field.
struct FieldVisitor {
    fields: Map<String, Value>,
    message: Option<String>,
    extract_message: bool,
}

impl FieldVisitor {
    fn for_event() -> Self {
        Self {
            fields: Map::new(),
            message: None,
            extract_message: true,
        }
    }

    fn for_span() -> Self {
        Self {
            fields: Map::new(),
            message: None,
            extract_message: false,
        }
    }

    fn insert(&mut self, name: &str, value: Value) {
        if self.extract_message && name == "message" {
            self.message = Some(match value {
                Value::String(text) => text,
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
            Some(number) => self.insert(field.name(), Value::Number(number)),
            None => self.insert(field.name(), Value::String(value.to_string())),
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.insert(field.name(), Value::String(format!("{:?}", value)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    fn collect_records(enabled: bool, emit: impl FnOnce()) -> Vec<LogMessage> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let layer = LogMessageLayer::new(tx, Arc::new(AtomicBool::new(enabled)));
        let _guard = tracing_subscriber::registry().with(layer).set_default();
        emit();
        let mut records = Vec::new();
        while let Ok(record) = rx.try_recv() {
            records.push(record);
        }
        records
    }

    #[test]
    fn events_carry_their_lazily_opened_span_chain() {
        let records = collect_records(true, || {
            let outer = tracing::info_span!("outer");
            let _outer = outer.enter();
            let inner = tracing::debug_span!("inner", package = "foo");
            let _inner = inner.enter();
            tracing::warn!(count = 3, "something happened");
        });

        let kinds: Vec<_> = records
            .iter()
            .map(|record| match record {
                LogMessage::SpanOpen(open) => format!("open:{}", open.name),
                LogMessage::SpanClose(_) => "close".to_string(),
                LogMessage::Event(event) => format!("event:{}", event.message),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "open:outer",
                "open:inner",
                "event:something happened",
                "close",
                "close"
            ]
        );

        let LogMessage::SpanOpen(outer) = &records[0] else {
            unreachable!()
        };
        let LogMessage::SpanOpen(inner) = &records[1] else {
            unreachable!()
        };
        let LogMessage::Event(event) = &records[2] else {
            unreachable!()
        };
        assert_eq!(outer.parent_id, None);
        assert_eq!(inner.parent_id, Some(outer.id));
        assert_eq!(
            inner.fields.get("package").and_then(Value::as_str),
            Some("foo")
        );
        assert_eq!(event.span_id, Some(inner.id));
        assert_eq!(event.fields.get("count").and_then(Value::as_i64), Some(3));
        assert_eq!(event.level, LogLevel::Warn);
    }

    #[test]
    fn info_events_and_eventless_spans_stay_off_the_wire() {
        let records = collect_records(true, || {
            let span = tracing::info_span!("quiet");
            let _span = span.enter();
            tracing::info!("this is build output, not a log record");
        });
        assert!(records.is_empty());
    }

    #[test]
    fn nothing_is_sent_before_activation() {
        let records = collect_records(false, || {
            tracing::error!("emitted before the frontend negotiated");
        });
        assert!(records.is_empty());
    }
}
