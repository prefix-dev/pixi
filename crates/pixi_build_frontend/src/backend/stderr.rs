use std::sync::Arc;

use crate::BackendOutputStream;
use pixi_build_types::log::{BACKEND_LOG_SENTINEL, BackendLogLevel, BackendLogRecord};
use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};

/// Stderr stream that captures the stderr output of the backend and stores it
/// in a buffer for later use. Lines tagged with [`BACKEND_LOG_SENTINEL`] are
/// parsed as structured backend log records and re-emitted through the
/// frontend's `tracing` subscriber; other lines are forwarded to `on_log`
/// unchanged.
pub(crate) async fn stream_stderr<W: BackendOutputStream>(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    cancel: oneshot::Receiver<()>,
    mut on_log: W,
) -> Result<String, std::io::Error> {
    // Create a future that continuously read from the buffer and stores the lines
    // until all data is received.
    let mut lines = Vec::new();
    let read_and_buffer = async {
        let mut buffer = buffer.lock().await;
        while let Some(line) = buffer.next_line().await? {
            match classify_line(&line) {
                ClassifiedLine::Record(record) => emit_record(&record),
                ClassifiedLine::Raw => {
                    on_log.on_line(line.clone());
                    lines.push(line);
                }
            }
        }
        Ok(lines.join("\n"))
    };

    // Either wait until the cancel signal is received or the `read_and_buffer`
    // finishes which means there is no more data to read.
    tokio::select! {
        _ = cancel => {
            Ok(lines.join("\n"))
        }
        result = read_and_buffer => {
            result
        }
    }
}

enum ClassifiedLine {
    Record(BackendLogRecord),
    Raw,
}

fn classify_line(line: &str) -> ClassifiedLine {
    // Malformed sentinel lines fall through to `Raw` so the user still sees
    // them rather than having them silently dropped.
    line.strip_prefix(BACKEND_LOG_SENTINEL)
        .and_then(|json| serde_json::from_str::<BackendLogRecord>(json).ok())
        .map(ClassifiedLine::Record)
        .unwrap_or(ClassifiedLine::Raw)
}

/// Re-emit a [`BackendLogRecord`] received from a backend as a `tracing` event
/// on the frontend side. Each name in [`BackendLogRecord::spans`] is opened
/// and entered as a frontend tracing span (innermost last) before the event
/// is emitted, so the frontend's subscriber sees the same hierarchy the
/// backend reported.
fn emit_record(record: &BackendLogRecord) {
    // `tracing::event!` requires a const level, so dispatch per branch. Fields
    // are flattened into a JSON string because tracing has no runtime field
    // API — readers who care about structure can re-parse `fields`.
    let fields = if record.fields.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&record.fields).unwrap_or_default()
    };
    let target = format!("pixi_build_backend::{}", record.target);

    // Open and enter a frontend span for each backend span name, innermost
    // last. Guards drop in reverse order when `_guards` goes out of scope, so
    // the spans close in the right order.
    let _guards: Vec<_> = record
        .spans
        .iter()
        .map(|name| dyn_span::enter(name))
        .collect();

    macro_rules! emit {
        ($lvl:expr) => {{
            tracing::event!(
                target: "pixi_build_backend",
                $lvl,
                backend.target = %target,
                backend.fields = %fields,
                "{}",
                record.message,
            );
        }};
    }

    match record.level {
        BackendLogLevel::Trace => emit!(tracing::Level::TRACE),
        BackendLogLevel::Debug => emit!(tracing::Level::DEBUG),
        BackendLogLevel::Info => emit!(tracing::Level::INFO),
        BackendLogLevel::Warn => emit!(tracing::Level::WARN),
        BackendLogLevel::Error => emit!(tracing::Level::ERROR),
    }
}

/// Runtime-named `tracing` spans, used to mirror the backend's span hierarchy.
///
/// `tracing`'s public macros require span names to be `&'static str` baked in
/// at the call site. To get arbitrary names through the dispatch system we
/// build the `Metadata`/`Callsite` pair by hand, leaking one per unique name.
/// Backends only have a small, bounded set of span names in practice, so the
/// per-process leak is negligible.
mod dyn_span {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };

    use tracing::{
        Metadata,
        callsite::{Callsite, Identifier},
        field::FieldSet,
        metadata::Kind,
        span::{EnteredSpan, Span},
    };

    pub(super) struct DynCallsite {
        name: &'static str,
        metadata: OnceLock<Metadata<'static>>,
    }

    impl DynCallsite {
        fn metadata_ref(&'static self) -> &'static Metadata<'static> {
            self.metadata.get_or_init(|| {
                Metadata::new(
                    self.name,
                    "pixi_build_backend",
                    tracing::Level::TRACE,
                    None,
                    None,
                    None,
                    FieldSet::new(&[], Identifier(self)),
                    Kind::SPAN,
                )
            })
        }
    }

    impl Callsite for DynCallsite {
        fn set_interest(&self, _: tracing::subscriber::Interest) {}
        fn metadata(&self) -> &Metadata<'_> {
            self.metadata
                .get()
                .expect("metadata initialised before first use")
        }
    }

    pub(super) fn callsite_for(name: &str) -> &'static DynCallsite {
        static INTERN: OnceLock<Mutex<HashMap<String, &'static DynCallsite>>> = OnceLock::new();
        let intern = INTERN.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = intern.lock().expect("dyn_span intern poisoned");
        if let Some(&cs) = guard.get(name) {
            return cs;
        }
        let leaked_name: &'static str = Box::leak(name.to_owned().into_boxed_str());
        let cs: &'static DynCallsite = Box::leak(Box::new(DynCallsite {
            name: leaked_name,
            metadata: OnceLock::new(),
        }));
        // Initialise the metadata before publishing the entry so `Callsite::metadata`
        // never observes an uninitialised slot.
        let _ = cs.metadata_ref();
        guard.insert(name.to_owned(), cs);
        cs
    }

    pub(super) fn enter(name: &str) -> EnteredSpan {
        let cs = callsite_for(name);
        let meta = cs.metadata_ref();
        let value_set = meta.fields().value_set(&[]);
        Span::new(meta, &value_set).entered()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_line_without_sentinel_is_forwarded() {
        assert!(matches!(
            classify_line("cargo: compiling foo"),
            ClassifiedLine::Raw
        ));
    }

    #[test]
    fn sentinel_line_with_valid_json_parses() {
        let payload = r#"{"level":"INFO","target":"pixi_build::recipe","message":"hello"}"#;
        let line = format!("{}{}", BACKEND_LOG_SENTINEL, payload);
        match classify_line(&line) {
            ClassifiedLine::Record(record) => {
                assert_eq!(record.level, BackendLogLevel::Info);
                assert_eq!(record.target, "pixi_build::recipe");
                assert_eq!(record.message, "hello");
            }
            ClassifiedLine::Raw => panic!("expected a structured record"),
        }
    }

    #[test]
    fn sentinel_line_with_malformed_json_falls_back_to_raw() {
        let line = format!("{}not json", BACKEND_LOG_SENTINEL);
        assert!(matches!(classify_line(&line), ClassifiedLine::Raw));
    }

    #[test]
    fn dyn_span_interns_by_name() {
        let a1 = dyn_span::callsite_for("build");
        let a2 = dyn_span::callsite_for("build");
        let b = dyn_span::callsite_for("render");
        assert!(std::ptr::eq(a1, a2), "same name should reuse the callsite");
        assert!(!std::ptr::eq(a1, b), "distinct names get distinct callsites");
    }

    #[test]
    fn dyn_span_can_be_entered_without_subscriber() {
        // Without a global subscriber the span is disabled, but constructing
        // and entering it must still be safe.
        let _g1 = dyn_span::enter("outer");
        let _g2 = dyn_span::enter("inner");
    }
}
