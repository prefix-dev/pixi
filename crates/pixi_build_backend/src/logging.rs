//! Forwarding the backend's own log events to the frontend.
//!
//! Backends log with `tracing`. Historically those events were formatted to
//! stderr and pixi scraped them back line by line, which loses the level, the
//! target and any structured fields. [`LogForwarder`] instead turns each event
//! into a `log/message` notification on the JSON-RPC connection.
//!
//! Whether that happens is decided at capability negotiation, so both layers
//! are installed up front and share a single [`LogForwarding`] switch: exactly
//! one of them is live at any moment, and events are never reported twice.

use std::{
    collections::BTreeMap,
    fmt::Debug,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use pixi_build_types::procedures::log::{self, LogLevel, LogParams};
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

use crate::stdio::MessageSender;

/// Decides where the backend's log events go.
///
/// Starts out disabled, which means stderr. Flipped once during capability
/// negotiation if the frontend advertised `providesLogNotifications`. Cloning
/// shares the switch.
#[derive(Clone, Debug, Default)]
pub struct LogForwarding(Arc<AtomicBool>);

impl LogForwarding {
    /// Route log events to the frontend as notifications from now on.
    pub fn enable(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether log events are being forwarded to the frontend.
    pub fn is_enabled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// A [`Layer`] that sends log events to the frontend as `log/message`
/// notifications.
pub struct LogForwarder {
    sender: MessageSender,
    forwarding: LogForwarding,
}

impl LogForwarder {
    /// Create the layer and the switch that arms it.
    pub fn new(sender: MessageSender) -> (Self, LogForwarding) {
        let forwarding = LogForwarding::default();
        let layer = Self {
            sender,
            forwarding: forwarding.clone(),
        };
        (layer, forwarding)
    }
}

impl<S> Layer<S> for LogForwarder
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if !self.forwarding.is_enabled() {
            return;
        }

        let mut visitor = CollectedFields::default();
        event.record(&mut visitor);

        let metadata = event.metadata();
        let params = LogParams {
            level: level_to_log_level(*metadata.level()),
            message: visitor.message.unwrap_or_default(),
            target: Some(metadata.target().to_string()),
            fields: visitor.fields,
        };

        // A failed send means the connection is gone. Reporting that would emit
        // another event, which would fail to send in exactly the same way.
        let _delivered = self.sender.notify(log::METHOD_NAME, &params);
    }
}

fn level_to_log_level(level: Level) -> LogLevel {
    match level {
        Level::TRACE => LogLevel::Trace,
        Level::DEBUG => LogLevel::Debug,
        Level::INFO => LogLevel::Info,
        Level::WARN => LogLevel::Warn,
        Level::ERROR => LogLevel::Error,
    }
}

/// Splits a `tracing` event into its message and its remaining fields.
#[derive(Default)]
struct CollectedFields {
    message: Option<String>,
    fields: BTreeMap<String, serde_json::Value>,
}

impl CollectedFields {
    fn insert(&mut self, field: &Field, value: serde_json::Value) {
        if field.name() == "message" {
            // `tracing` puts the format string in a field called `message`; the
            // protocol carries it separately.
            self.message = Some(match value {
                serde_json::Value::String(message) => message,
                other => other.to_string(),
            });
        } else {
            self.fields.insert(field.name().to_string(), value);
        }
    }
}

impl Visit for CollectedFields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.insert(field, value.into());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.insert(field, format!("{value:?}").into());
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tracing::field::Visit;
    use tracing_subscriber::layer::SubscriberExt;

    use super::{CollectedFields, Level, LogForwarder, LogLevel, level_to_log_level};
    use crate::stdio::{channel_to, shutdown};

    /// The whole point of the change: an ordinary `tracing` call in a backend
    /// reaches pixi as a structured notification, with the level, the target
    /// and the fields intact rather than flattened into a stderr line.
    #[tokio::test]
    async fn events_become_notifications_once_forwarding_is_enabled() {
        let (sink, collected) = tokio::io::duplex(8 * 1024);
        let (sender, incoming) = channel_to(sink);
        let (layer, forwarding) = LogForwarder::new(sender);

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            // Before negotiation nothing may go over the wire: the frontend has
            // not said it understands these notifications yet.
            tracing::info!(package = "libfoo", "before negotiation");

            forwarding.enable();
            tracing::warn!(package = "libfoo", "after negotiation");
        });

        shutdown(incoming).await;

        let mut lines = Vec::new();
        let mut reader = BufReader::new(collected).lines();
        while let Some(line) = reader
            .next_line()
            .await
            .expect("reading the sink cannot fail")
        {
            lines.push(line);
        }

        assert_eq!(
            lines.len(),
            1,
            "only the event after negotiation may be forwarded: {lines:?}"
        );

        let notification: serde_json::Value =
            serde_json::from_str(&lines[0]).expect("a notification is valid JSON");
        assert_eq!(notification["method"], "log/message");
        assert_eq!(notification["params"]["level"], "warn");
        assert_eq!(notification["params"]["message"], "after negotiation");
        assert_eq!(notification["params"]["fields"]["package"], "libfoo");
        assert_eq!(
            notification["params"]["target"], "pixi_build_backend::logging::tests",
            "the originating module is what stderr scraping could never recover"
        );
    }

    /// The message is what a user reads, so it must not end up quoted the way
    /// `record_debug` would render a string.
    #[test]
    fn message_field_is_extracted_unquoted() {
        let callsite = tracing::field::FieldSet::new(
            &["message", "package"],
            tracing::callsite::Identifier(&TEST_CALLSITE),
        );
        let mut fields = callsite.iter();
        let message = fields.next().expect("the field set has two fields");
        let package = fields.next().expect("the field set has two fields");

        let mut collected = CollectedFields::default();
        collected.record_str(&message, "building libfoo");
        collected.record_str(&package, "libfoo");

        assert_eq!(collected.message.as_deref(), Some("building libfoo"));
        assert_eq!(
            collected.fields.get("package"),
            Some(&serde_json::json!("libfoo")),
            "non-message fields stay in the structured map"
        );
        assert!(
            !collected.fields.contains_key("message"),
            "the message must not be duplicated into the fields map"
        );
    }

    /// Every `tracing` level maps onto a protocol level; a wrong mapping would
    /// silently downgrade errors in pixi's output.
    #[test]
    fn levels_map_across_the_wire() {
        assert_eq!(level_to_log_level(Level::TRACE), LogLevel::Trace);
        assert_eq!(level_to_log_level(Level::DEBUG), LogLevel::Debug);
        assert_eq!(level_to_log_level(Level::INFO), LogLevel::Info);
        assert_eq!(level_to_log_level(Level::WARN), LogLevel::Warn);
        assert_eq!(level_to_log_level(Level::ERROR), LogLevel::Error);
    }

    struct TestCallsite;
    impl tracing::Callsite for TestCallsite {
        fn set_interest(&self, _interest: tracing::subscriber::Interest) {}
        fn metadata(&self) -> &tracing::Metadata<'_> {
            unreachable!("the test only needs field identity")
        }
    }
    static TEST_CALLSITE: TestCallsite = TestCallsite;
}
