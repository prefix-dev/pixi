//! Process-scoped pump for the backend's stderr stream.
//!
//! A single [`StderrPump`] task is spawned as soon as the backend process
//! starts (before `negotiate_capabilities` is called) and runs until the
//! process exits. It owns the backend's span lifecycle state for the whole
//! process lifetime — which matters because backend `tracing` span ids are
//! process-scoped, so records arriving during setup, between RPC calls, or
//! after a response can still reference a span that was opened earlier.
//!
//! Per-RPC code subscribes to the pump for the duration of a call to
//! capture raw build output through a [`BackendOutputStream`]; sentinel-tagged
//! log records are always processed regardless of whether any subscriber is
//! active.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use crate::BackendOutputStream;
use pixi_build_types::log::{
    BACKEND_LOG_SENTINEL, BackendEventRecord, BackendLogLevel, BackendLogRecord,
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::ChildStderr,
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};
use tracing::Span;

pub(crate) struct StderrPump {
    /// Active subscribers receiving raw (non-sentinel) lines. Dead senders
    /// (whose receiver has been dropped) are removed lazily by the pump
    /// loop, so callers don't need to deregister explicitly.
    subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>>,
    /// Handle to the pump task. Aborted on drop so an orphaned backend
    /// process doesn't leave the pump spinning.
    task: StdMutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for StderrPump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StderrPump").finish_non_exhaustive()
    }
}

impl StderrPump {
    pub(crate) fn spawn(stderr: ChildStderr) -> Arc<Self> {
        let subscribers: Arc<Mutex<Vec<mpsc::UnboundedSender<String>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let subscribers_for_task = subscribers.clone();
        let task = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            let mut span_map: HashMap<u64, Span> = HashMap::new();
            while let Ok(Some(line)) = reader.next_line().await {
                match classify_line(&line) {
                    ClassifiedLine::Record(record) => apply_record(record, &mut span_map),
                    ClassifiedLine::Raw => {
                        let mut subs = subscribers_for_task.lock().await;
                        if subs.is_empty() {
                            continue;
                        }
                        // Send to each active subscriber; drop dead ones in
                        // place so future iterations don't pay for them.
                        subs.retain(|sender| sender.send(line.clone()).is_ok());
                    }
                }
            }
            // Backend stderr closed (process exited). Dropping `span_map`
            // here closes any spans that the backend forgot to close.
        });
        Arc::new(Self {
            subscribers,
            task: StdMutex::new(Some(task)),
        })
    }

    /// Subscribe to raw lines for the duration of `cancel`, forwarding each
    /// to `sink` and accumulating them into the returned string for error
    /// reporting (matching the previous `stream_stderr` contract).
    pub(crate) async fn run_with_sink<W>(
        self: Arc<Self>,
        mut sink: W,
        cancel: oneshot::Receiver<()>,
    ) -> Result<String, std::io::Error>
    where
        W: BackendOutputStream + Send + 'static,
    {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        self.subscribers.lock().await.push(sender);

        let mut lines = Vec::new();
        let read_loop = async {
            while let Some(line) = receiver.recv().await {
                sink.on_line(line.clone());
                lines.push(line);
            }
            Ok::<String, std::io::Error>(lines.join("\n"))
        };

        let result = tokio::select! {
            _ = cancel => Ok(lines.join("\n")),
            r = read_loop => r,
        };
        // Dropping the receiver makes the pump's next send fail, which
        // removes our sender from the subscriber list automatically.
        drop(receiver);
        result
    }
}

impl Drop for StderrPump {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.task.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
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
            let target = format!("pixi_build_backend::{}", open.target);
            let span = dyn_span::create(
                &open.name,
                &target,
                backend_to_tracing_level(open.level),
                parent_id,
            );
            spans.insert(open.id, span);
        }
        BackendLogRecord::SpanClose(close) => {
            spans.remove(&close.id);
        }
        BackendLogRecord::Event(event) => {
            let parent = event.span_id.and_then(|id| spans.get(&id));
            emit_event(&event, parent);
        }
    }
}

fn backend_to_tracing_level(level: BackendLogLevel) -> tracing::Level {
    match level {
        BackendLogLevel::Trace => tracing::Level::TRACE,
        BackendLogLevel::Debug => tracing::Level::DEBUG,
        BackendLogLevel::Info => tracing::Level::INFO,
        BackendLogLevel::Warn => tracing::Level::WARN,
        BackendLogLevel::Error => tracing::Level::ERROR,
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
/// `tracing`'s public macros require span names, targets, and levels to be
/// `&'static` at the call site. To get arbitrary runtime values through the
/// dispatch system we build the `Metadata`/`Callsite` pair by hand, leaking
/// one per unique `(target, name, level)` tuple. Backends only emit a small,
/// bounded set of span shapes in practice.
///
/// Keying on level (not just name) is important: filters like
/// `pixi_build_backend=info` would otherwise drop INFO/WARN backend spans
/// whose dyn callsite was created at TRACE, defeating the entire reason for
/// reconstructing them.
mod dyn_span {
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };

    use tracing::{
        Level, Metadata,
        callsite::{Callsite, Identifier},
        field::FieldSet,
        metadata::Kind,
        span::{Id, Span},
    };

    pub(super) struct DynCallsite {
        name: &'static str,
        target: &'static str,
        level: Level,
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

    fn level_as_u8(level: Level) -> u8 {
        match level {
            Level::TRACE => 0,
            Level::DEBUG => 1,
            Level::INFO => 2,
            Level::WARN => 3,
            Level::ERROR => 4,
        }
    }

    pub(super) fn callsite_for(name: &str, target: &str, level: Level) -> &'static DynCallsite {
        type Key = (String, String, u8);
        static INTERN: OnceLock<Mutex<HashMap<Key, &'static DynCallsite>>> = OnceLock::new();
        let intern = INTERN.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = intern.lock().expect("dyn_span intern poisoned");
        let key = (target.to_string(), name.to_string(), level_as_u8(level));
        if let Some(&cs) = guard.get(&key) {
            return cs;
        }
        let leaked_name: &'static str = Box::leak(name.to_owned().into_boxed_str());
        let leaked_target: &'static str = Box::leak(target.to_owned().into_boxed_str());
        let cs: &'static DynCallsite = Box::leak(Box::new(DynCallsite {
            name: leaked_name,
            target: leaked_target,
            level,
            metadata: OnceLock::new(),
        }));
        let _ = cs.metadata_ref();
        guard.insert(key, cs);
        cs
    }

    pub(super) fn create(name: &str, target: &str, level: Level, parent: Option<Id>) -> Span {
        let cs = callsite_for(name, target, level);
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
        assert_eq!(open.target, "t");
        assert_eq!(open.level, BackendLogLevel::Info);
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

        apply_record(
            BackendLogRecord::SpanClose(BackendSpanCloseRecord { id: 2 }),
            &mut spans,
        );
        assert!(spans.contains_key(&1));
        assert!(!spans.contains_key(&2));

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
    fn dyn_span_interns_by_full_key() {
        let a1 = dyn_span::callsite_for("build", "t", tracing::Level::INFO);
        let a2 = dyn_span::callsite_for("build", "t", tracing::Level::INFO);
        let b_target = dyn_span::callsite_for("build", "other", tracing::Level::INFO);
        let b_level = dyn_span::callsite_for("build", "t", tracing::Level::DEBUG);
        let b_name = dyn_span::callsite_for("render", "t", tracing::Level::INFO);
        assert!(std::ptr::eq(a1, a2));
        assert!(!std::ptr::eq(a1, b_target));
        assert!(!std::ptr::eq(a1, b_level));
        assert!(!std::ptr::eq(a1, b_name));
    }
}
