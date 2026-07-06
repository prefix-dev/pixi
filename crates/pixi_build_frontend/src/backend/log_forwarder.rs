//! Re-emits `log/message` notifications received from a build backend
//! through the frontend's own `tracing` subscriber.
//!
//! The backend mirrors its span hierarchy over the wire (see
//! [`pixi_build_types::procedures::log_message`]); this module reconstructs
//! it with real `tracing` spans so events render with their full
//! `outer:inner:` context and are subject to the frontend's own filtering.
//! Targets are prefixed with `backend::<name>::` (e.g.
//! `backend::pixi-build-python::rattler_build_core::packaging`) so backend
//! records are distinguishable from the frontend's, name which backend
//! produced them, and can be filtered as a group (`RUST_LOG=backend=trace`)
//! or per backend (`RUST_LOG=backend::pixi-build-python=trace`).
//!
//! `tracing`'s macros require metadata to be `&'static`, which is impossible
//! for names that only exist at runtime. The [`callsites`] module below
//! therefore interns one leaked callsite per unique `(name, target, level)`
//! tuple; the set is bounded by the number of distinct callsites in the
//! backend, which is small and stable in practice.

use std::collections::HashMap;

use pixi_build_types::procedures::log_message::{LogEvent, LogLevel, LogMessage, LogSpanOpen};
use tracing::{Level, Span};

/// Prefix prepended to the backend's `tracing` targets on re-emission.
const TARGET_PREFIX: &str = "backend::";

/// Process-scoped state: maps the backend's span ids to the frontend spans
/// mirroring them. Lives for the lifetime of the backend process, so span
/// ids stay valid across RPC calls.
pub(crate) struct LogForwarder {
    /// `backend::<name>::` prepended to every re-emitted target so records are
    /// attributed to the backend that produced them.
    target_prefix: String,
    spans: HashMap<u64, Span>,
}

impl LogForwarder {
    pub(crate) fn new(backend_name: impl AsRef<str>) -> Self {
        Self {
            target_prefix: format!("{TARGET_PREFIX}{}::", backend_name.as_ref()),
            spans: HashMap::new(),
        }
    }

    pub(crate) fn apply(&mut self, record: LogMessage) {
        match record {
            LogMessage::SpanOpen(open) => self.open_span(open),
            LogMessage::SpanClose(close) => {
                self.spans.remove(&close.id);
            }
            LogMessage::Event(event) => {
                let parent = event.span_id.and_then(|id| self.spans.get(&id).cloned());
                self.emit_event(&event, parent.as_ref());
            }
        }
    }

    fn open_span(&mut self, open: LogSpanOpen) {
        let parent = open.parent_id.and_then(|id| self.spans.get(&id));
        let span = callsites::new_span(
            &open.name,
            &format!("{}{}", self.target_prefix, open.target),
            level_from_wire(open.level),
            parent.and_then(Span::id),
        );
        // If the frontend's filter disabled this span, fall back to its
        // parent so descendants keep the deepest *enabled* ancestor as
        // context — mirroring how `tracing` treats disabled spans natively.
        let span = if span.is_disabled() {
            parent.cloned().unwrap_or(span)
        } else {
            span
        };
        self.spans.insert(open.id, span);
    }

    fn emit_event(&self, event: &LogEvent, parent: Option<&Span>) {
        let mut message = event.message.clone();
        for (key, value) in &event.fields {
            use std::fmt::Write;
            let _ = match value {
                serde_json::Value::String(text) => write!(message, " {key}={text}"),
                other => write!(message, " {key}={other}"),
            };
        }
        callsites::emit_event(
            &format!("{}{}", self.target_prefix, event.target),
            level_from_wire(event.level),
            parent.and_then(Span::id),
            &message,
        );
    }
}

fn level_from_wire(level: LogLevel) -> Level {
    match level {
        LogLevel::Trace => Level::TRACE,
        LogLevel::Debug => Level::DEBUG,
        LogLevel::Info => Level::INFO,
        LogLevel::Warn => Level::WARN,
        LogLevel::Error => Level::ERROR,
    }
}

/// Runtime-constructed `tracing` callsites.
///
/// One callsite (and its `Metadata`) is leaked per unique
/// `(kind, name, target, level)` key. Keying on the level matters: interest
/// caching and `EnvFilter` evaluate a callsite's metadata, so reusing a
/// callsite created at a different level would make filtering decisions with
/// the wrong level.
mod callsites {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };

    use tracing::{
        Event, Level, Metadata,
        callsite::{Callsite, Identifier},
        field::FieldSet,
        metadata::Kind,
        span::{Id, Span},
    };

    pub(super) struct DynCallsite {
        name: &'static str,
        target: &'static str,
        level: Level,
        is_span: bool,
        fields: &'static [&'static str],
        metadata: OnceLock<Metadata<'static>>,
    }

    impl DynCallsite {
        fn metadata_ref(&'static self) -> &'static Metadata<'static> {
            self.metadata.get_or_init(|| {
                Metadata::new(
                    self.name,
                    self.target,
                    self.level,
                    None,
                    None,
                    None,
                    FieldSet::new(self.fields, Identifier(self)),
                    if self.is_span {
                        Kind::SPAN
                    } else {
                        Kind::EVENT
                    },
                )
            })
        }
    }

    impl Callsite for DynCallsite {
        fn set_interest(&self, _: tracing::subscriber::Interest) {}
        fn metadata(&self) -> &Metadata<'_> {
            self.metadata
                .get()
                .expect("metadata is initialised at interning time")
        }
    }

    #[derive(PartialEq, Eq, Hash)]
    struct Key {
        is_span: bool,
        name: String,
        target: String,
        level: Level,
    }

    fn intern(key: Key) -> &'static DynCallsite {
        static INTERNED: OnceLock<Mutex<HashMap<Key, &'static DynCallsite>>> = OnceLock::new();
        let mut interned = INTERNED
            .get_or_init(Default::default)
            .lock()
            .expect("callsite interner is poisoned");
        if let Some(&callsite) = interned.get(&key) {
            return callsite;
        }
        let callsite: &'static DynCallsite = Box::leak(Box::new(DynCallsite {
            name: Box::leak(key.name.clone().into_boxed_str()),
            target: Box::leak(key.target.clone().into_boxed_str()),
            level: key.level,
            is_span: key.is_span,
            fields: if key.is_span { &[] } else { &["message"] },
            metadata: OnceLock::new(),
        }));
        let _ = callsite.metadata_ref();
        tracing::callsite::register(callsite);
        interned.insert(key, callsite);
        callsite
    }

    pub(super) fn span_callsite(name: &str, target: &str, level: Level) -> &'static DynCallsite {
        intern(Key {
            is_span: true,
            name: name.to_owned(),
            target: target.to_owned(),
            level,
        })
    }

    fn event_callsite(target: &str, level: Level) -> &'static DynCallsite {
        intern(Key {
            is_span: false,
            name: "backend event".to_owned(),
            target: target.to_owned(),
            level,
        })
    }

    /// Create a span mirroring a backend span. Returns a disabled span when
    /// the frontend's subscriber is not interested in it.
    pub(super) fn new_span(name: &str, target: &str, level: Level, parent: Option<Id>) -> Span {
        let metadata = span_callsite(name, target, level).metadata_ref();
        // The `tracing` macros perform this check before constructing a
        // span; going through `Span::child_of` directly, we must do it
        // ourselves or filtering would be bypassed. This must be a separate
        // `get_default` call: `Span::child_of` dispatches internally, and a
        // nested `get_default` would silently hit the no-op subscriber.
        if !tracing::dispatcher::get_default(|dispatch| dispatch.enabled(metadata)) {
            return Span::none();
        }
        let values = metadata.fields().value_set(&[]);
        Span::child_of(parent, metadata, &values)
    }

    /// Emit an event mirroring a backend event, if the frontend's subscriber
    /// is interested in it.
    pub(super) fn emit_event(target: &str, level: Level, parent: Option<Id>, message: &str) {
        let metadata = event_callsite(target, level).metadata_ref();
        // Note: everything happens inside a single `get_default` closure —
        // nesting (e.g. through `Event::child_of`, which calls `get_default`
        // internally) would trip the dispatcher's re-entrancy protection and
        // silently dispatch to the no-op subscriber.
        tracing::dispatcher::get_default(move |dispatch| {
            // The `tracing` macros perform this check before constructing an
            // event; going through the dispatcher directly, we must do it
            // ourselves or filtering would be bypassed.
            if !dispatch.enabled(metadata) {
                return;
            }
            let fields = metadata.fields();
            let message_field = fields
                .field("message")
                .expect("event callsites always have a message field");
            let values = [(&message_field, Some(&message as &dyn tracing::field::Value))];
            let values = fields.value_set(&values);
            let event = Event::new_child_of(parent.clone(), metadata, &values);
            dispatch.event(&event);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    /// Captures re-emitted events together with their full span context.
    #[derive(Clone, Default)]
    struct Capture {
        events: Arc<Mutex<Vec<String>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for Capture
    where
        S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct MessageVisitor(String);
            impl tracing::field::Visit for MessageVisitor {
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    if field.name() == "message" {
                        self.0 = value.to_owned();
                    }
                }

                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    if field.name() == "message" {
                        self.0 = format!("{value:?}");
                    }
                }
            }
            let mut visitor = MessageVisitor(String::new());
            event.record(&mut visitor);

            let scope = ctx
                .event_scope(event)
                .into_iter()
                .flat_map(|scope| scope.from_root())
                .map(|span| span.name().to_owned())
                .collect::<Vec<_>>()
                .join(":");
            let level = *event.metadata().level();
            let target = event.metadata().target().to_owned();
            self.events
                .lock()
                .unwrap()
                .push(format!("{level} {target} [{scope}] {}", visitor.0));
        }
    }

    fn span_open(id: u64, parent_id: Option<u64>, name: &str) -> LogMessage {
        LogMessage::SpanOpen(LogSpanOpen {
            id,
            parent_id,
            level: LogLevel::Info,
            target: "rattler_build_core::build".to_string(),
            name: name.to_string(),
            fields: Default::default(),
        })
    }

    fn event(span_id: Option<u64>, level: LogLevel, message: &str) -> LogMessage {
        LogMessage::Event(LogEvent {
            level,
            target: "rattler_build_core::build".to_string(),
            message: message.to_string(),
            fields: Default::default(),
            span_id,
        })
    }

    #[test]
    fn events_re_emit_with_reconstructed_span_hierarchy() {
        let capture = Capture::default();
        let _guard = tracing_subscriber::registry()
            .with(capture.clone())
            .set_default();

        let mut forwarder = LogForwarder::new("pixi-build-rattler-build");
        forwarder.apply(span_open(1, None, "outer"));
        forwarder.apply(span_open(2, Some(1), "inner"));
        forwarder.apply(event(Some(2), LogLevel::Warn, "nested"));
        forwarder.apply(LogMessage::SpanClose(
            pixi_build_types::procedures::log_message::LogSpanClose { id: 2 },
        ));
        forwarder.apply(event(Some(1), LogLevel::Error, "after close"));
        forwarder.apply(event(Some(999), LogLevel::Warn, "unknown span"));
        forwarder.apply(event(None, LogLevel::Debug, "no span"));

        let events = capture.events.lock().unwrap();
        assert_eq!(
            *events,
            [
                "WARN backend::pixi-build-rattler-build::rattler_build_core::build [outer:inner] nested",
                "ERROR backend::pixi-build-rattler-build::rattler_build_core::build [outer] after close",
                "WARN backend::pixi-build-rattler-build::rattler_build_core::build [] unknown span",
                "DEBUG backend::pixi-build-rattler-build::rattler_build_core::build [] no span",
            ]
        );
    }

    #[test]
    fn fields_are_folded_into_the_message() {
        let capture = Capture::default();
        let _guard = tracing_subscriber::registry()
            .with(capture.clone())
            .set_default();

        let mut fields = serde_json::Map::new();
        fields.insert("path".to_string(), serde_json::Value::from("foo/bar"));
        fields.insert("count".to_string(), serde_json::Value::from(3));
        LogForwarder::new("pixi-build-rattler-build").apply(LogMessage::Event(LogEvent {
            level: LogLevel::Warn,
            target: "t".to_string(),
            message: "base".to_string(),
            fields,
            span_id: None,
        }));

        let events = capture.events.lock().unwrap();
        assert_eq!(
            *events,
            ["WARN backend::pixi-build-rattler-build::t [] base path=foo/bar count=3"]
        );
    }

    #[test]
    fn callsites_are_interned_by_name_target_and_level() {
        let a1 = callsites::span_callsite("build", "t", Level::INFO);
        let a2 = callsites::span_callsite("build", "t", Level::INFO);
        let by_target = callsites::span_callsite("build", "other", Level::INFO);
        let by_level = callsites::span_callsite("build", "t", Level::DEBUG);
        let by_name = callsites::span_callsite("render", "t", Level::INFO);
        assert!(std::ptr::eq(a1, a2));
        assert!(!std::ptr::eq(a1, by_target));
        assert!(!std::ptr::eq(a1, by_level));
        assert!(!std::ptr::eq(a1, by_name));
    }
}
