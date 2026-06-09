use std::{collections::HashMap, sync::Arc};

use crate::BackendOutputStream;
use pixi_build_types::log::{
    BACKEND_LOG_SENTINEL, BackendEventRecord, BackendLogLevel, BackendLogRecord,
};
use tokio::{
    io::{BufReader, Lines},
    process::ChildStderr,
    sync::{Mutex, oneshot},
};
use tracing::Span;

/// Stderr stream that captures the stderr output of the backend and stores it
/// in a buffer for later use. Lines tagged with [`BACKEND_LOG_SENTINEL`] are
/// parsed as structured backend log records and replayed through the
/// frontend's `tracing` subscriber, preserving the backend's span hierarchy;
/// other lines are forwarded to `on_log` unchanged.
pub(crate) async fn stream_stderr<W: BackendOutputStream>(
    buffer: Arc<Mutex<Lines<BufReader<ChildStderr>>>>,
    cancel: oneshot::Receiver<()>,
    mut on_log: W,
) -> Result<String, std::io::Error> {
    let mut lines = Vec::new();
    let read_and_buffer = async {
        let mut buffer = buffer.lock().await;
        let mut spans: HashMap<u64, Span> = HashMap::new();
        while let Some(line) = buffer.next_line().await? {
            match classify_line(&line) {
                ClassifiedLine::Record(record) => apply_record(record, &mut spans),
                ClassifiedLine::Raw => {
                    on_log.on_line(line.clone());
                    lines.push(line);
                }
            }
        }
        // Spans drop here in arbitrary order — fine, because the backend
        // process has exited and there's no one left to receive close events.
        Ok(lines.join("\n"))
    };

    tokio::select! {
        _ = cancel => Ok(lines.join("\n")),
        result = read_and_buffer => result,
    }
}

enum ClassifiedLine {
    Record(BackendLogRecord),
    Raw,
}

fn classify_line(line: &str) -> ClassifiedLine {
    line.strip_prefix(BACKEND_LOG_SENTINEL)
        .and_then(|json| serde_json::from_str::<BackendLogRecord>(json).ok())
        .map(ClassifiedLine::Record)
        .unwrap_or(ClassifiedLine::Raw)
}

fn apply_record(record: BackendLogRecord, spans: &mut HashMap<u64, Span>) {
    match record {
        BackendLogRecord::SpanOpen(open) => {
            let parent_id = open
                .parent_id
                .and_then(|p| spans.get(&p))
                .and_then(Span::id);
            let span = dyn_span::create(&open.name, parent_id);
            spans.insert(open.id, span);
        }
        BackendLogRecord::SpanClose(close) => {
            // Dropping the Span here signals close to the subscriber. Late
            // events referencing a closed id will simply emit without a
            // parent.
            spans.remove(&close.id);
        }
        BackendLogRecord::Event(event) => {
            let parent = event.span_id.and_then(|id| spans.get(&id));
            emit_event(&event, parent);
        }
    }
}

/// Emit a backend event through the frontend's `tracing` dispatcher, scoped
/// inside `parent` if known. `tracing::event!` needs a const level, so we
/// dispatch via a small per-level macro.
fn emit_event(event: &BackendEventRecord, parent: Option<&Span>) {
    let fields = if event.fields.is_empty() {
        String::new()
    } else {
        serde_json::to_string(&event.fields).unwrap_or_default()
    };
    let target = format!("pixi_build_backend::{}", event.target);

    macro_rules! emit {
        ($lvl:expr) => {{
            tracing::event!(
                target: "pixi_build_backend",
                $lvl,
                backend.target = %target,
                backend.fields = %fields,
                "{}",
                event.message,
            );
        }};
    }

    let do_emit = || match event.level {
        BackendLogLevel::Trace => emit!(tracing::Level::TRACE),
        BackendLogLevel::Debug => emit!(tracing::Level::DEBUG),
        BackendLogLevel::Info => emit!(tracing::Level::INFO),
        BackendLogLevel::Warn => emit!(tracing::Level::WARN),
        BackendLogLevel::Error => emit!(tracing::Level::ERROR),
    };

    match parent {
        Some(span) => span.in_scope(do_emit),
        None => do_emit(),
    }
}

/// Runtime-named `tracing` spans, used to mirror the backend's span hierarchy.
///
/// `tracing`'s public macros require span names to be `&'static str` baked in
/// at the call site. To get arbitrary names through the dispatch system we
/// build the `Metadata`/`Callsite` pair by hand, leaking one per unique name.
/// Backends only have a small, bounded set of span names in practice, so the
/// per-process leak is negligible — and lifecycle propagation means we open
/// each span once per actual lifetime rather than once per event.
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
        span::{Id, Span},
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
        let _ = cs.metadata_ref();
        guard.insert(name.to_owned(), cs);
        cs
    }

    pub(super) fn create(name: &str, parent: Option<Id>) -> Span {
        let cs = callsite_for(name);
        let meta = cs.metadata_ref();
        let value_set = meta.fields().value_set(&[]);
        Span::child_of(parent, meta, &value_set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_build_types::log::{BackendEventRecord, BackendSpanCloseRecord, BackendSpanOpenRecord};

    #[test]
    fn raw_line_without_sentinel_is_forwarded() {
        assert!(matches!(
            classify_line("cargo: compiling foo"),
            ClassifiedLine::Raw
        ));
    }

    #[test]
    fn sentinel_event_line_parses() {
        let payload = r#"{"kind":"event","level":"INFO","target":"pixi_build::recipe","message":"hello"}"#;
        let line = format!("{}{}", BACKEND_LOG_SENTINEL, payload);
        let ClassifiedLine::Record(BackendLogRecord::Event(event)) = classify_line(&line) else {
            panic!("expected an event record");
        };
        assert_eq!(event.level, BackendLogLevel::Info);
        assert_eq!(event.target, "pixi_build::recipe");
        assert_eq!(event.message, "hello");
    }

    #[test]
    fn sentinel_span_open_line_parses() {
        let payload = r#"{"kind":"span_open","id":3,"parent_id":1,"level":"INFO","target":"t","name":"build"}"#;
        let line = format!("{}{}", BACKEND_LOG_SENTINEL, payload);
        let ClassifiedLine::Record(BackendLogRecord::SpanOpen(open)) = classify_line(&line) else {
            panic!("expected a span_open record");
        };
        assert_eq!(open.id, 3);
        assert_eq!(open.parent_id, Some(1));
        assert_eq!(open.name, "build");
    }

    #[test]
    fn malformed_sentinel_line_falls_back_to_raw() {
        let line = format!("{}not json", BACKEND_LOG_SENTINEL);
        assert!(matches!(classify_line(&line), ClassifiedLine::Raw));
    }

    #[test]
    fn applying_lifecycle_records_inserts_and_removes_spans() {
        let mut spans: HashMap<u64, Span> = HashMap::new();

        apply_record(
            BackendLogRecord::SpanOpen(BackendSpanOpenRecord {
                id: 1,
                parent_id: None,
                level: BackendLogLevel::Info,
                target: "t".into(),
                name: "outer".into(),
                fields: Default::default(),
                timestamp: None,
            }),
            &mut spans,
        );
        apply_record(
            BackendLogRecord::SpanOpen(BackendSpanOpenRecord {
                id: 2,
                parent_id: Some(1),
                level: BackendLogLevel::Info,
                target: "t".into(),
                name: "inner".into(),
                fields: Default::default(),
                timestamp: None,
            }),
            &mut spans,
        );
        assert!(spans.contains_key(&1));
        assert!(spans.contains_key(&2));

        // Event referencing an existing span — must not panic.
        apply_record(
            BackendLogRecord::Event(BackendEventRecord {
                level: BackendLogLevel::Info,
                target: "t".into(),
                message: "m".into(),
                fields: Default::default(),
                timestamp: None,
                span_id: Some(2),
            }),
            &mut spans,
        );

        // Closing an inner span removes only that entry.
        apply_record(
            BackendLogRecord::SpanClose(BackendSpanCloseRecord { id: 2 }),
            &mut spans,
        );
        assert!(spans.contains_key(&1));
        assert!(!spans.contains_key(&2));

        // Event referencing a closed span — must not panic; emitted parentless.
        apply_record(
            BackendLogRecord::Event(BackendEventRecord {
                level: BackendLogLevel::Warn,
                target: "t".into(),
                message: "late".into(),
                fields: Default::default(),
                timestamp: None,
                span_id: Some(2),
            }),
            &mut spans,
        );
    }

    #[test]
    fn dyn_span_interns_by_name() {
        let a1 = dyn_span::callsite_for("build");
        let a2 = dyn_span::callsite_for("build");
        let b = dyn_span::callsite_for("render");
        assert!(std::ptr::eq(a1, a2), "same name should reuse the callsite");
        assert!(!std::ptr::eq(a1, b), "distinct names get distinct callsites");
    }
}
