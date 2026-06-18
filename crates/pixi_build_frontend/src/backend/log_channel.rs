//! Frontend side of the structured log channel.
//!
//! Before spawning the backend the frontend creates a [`LogListener`]
//! (Unix socket on Unix, named pipe on Windows) and hands its address to
//! the backend via [`BACKEND_LOG_SOCKET_ENV`]. Once the backend connects,
//! [`LogListener::accept`] returns a [`LogPump`] handle whose background
//! task drains JSON records line-by-line, replaying span lifetimes and
//! events through the frontend's `tracing` subscriber.
//!
//! Because the log channel is a dedicated transport, it never interleaves
//! with the backend's raw build output on stderr — and it survives
//! across the whole backend process lifetime, so span ids remain valid
//! between RPC calls and during setup/teardown.

use std::{
    collections::HashMap,
    pin::Pin,
    sync::Mutex as StdMutex,
};

use pixi_build_types::log::{
    BackendEventRecord, BackendLogLevel, BackendLogRecord,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    task::JoinHandle,
};
use tracing::Span;

/// Address advertised to the backend through [`BACKEND_LOG_SOCKET_ENV`].
pub(crate) type LogChannelAddr = String;

/// Listens for the backend's incoming log connection.
pub(crate) struct LogListener {
    inner: PlatformListener,
    pub(crate) addr: LogChannelAddr,
}

impl LogListener {
    pub(crate) fn create() -> std::io::Result<Self> {
        let (inner, addr) = PlatformListener::create()?;
        Ok(Self { inner, addr })
    }

    /// Wait for the backend to connect, then return a pump that drains the
    /// connection in the background.
    pub(crate) async fn accept(self) -> std::io::Result<LogPump> {
        let reader = self.inner.accept().await?;
        let task = tokio::spawn(pump_loop(reader));
        Ok(LogPump {
            task: StdMutex::new(Some(task)),
        })
    }
}

/// Background task draining the log channel. Aborts on drop.
pub(crate) struct LogPump {
    task: StdMutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for LogPump {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogPump").finish_non_exhaustive()
    }
}

impl Drop for LogPump {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.task.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
    }
}

async fn pump_loop(reader: Pin<Box<dyn AsyncRead + Send>>) {
    let mut lines = BufReader::new(reader).lines();
    let mut spans: HashMap<u64, Span> = HashMap::new();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(record) = serde_json::from_str::<BackendLogRecord>(&line) else {
            continue;
        };
        apply_record(record, &mut spans);
    }
    // Backend connection closed (process exited / socket EOF). Dropping
    // `spans` closes any spans the backend forgot to close.
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

// ---------------------------------------------------------------------------
// Platform-specific listener
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod platform {
    use std::path::PathBuf;

    use tokio::{
        io::AsyncRead,
        net::UnixListener,
    };

    /// Unix socket listener. Removes the temp socket file on drop.
    pub(super) struct PlatformListener {
        listener: UnixListener,
        path: PathBuf,
    }

    impl PlatformListener {
        pub(super) fn create() -> std::io::Result<(Self, String)> {
            // Build a unique path in the system temp dir. We deliberately
            // don't use tempfile::NamedTempFile because we want the path
            // free before bind() — the kernel rejects bind on an existing
            // file.
            let path = std::env::temp_dir().join(format!(
                "pixi-build-log-{}-{:x}.sock",
                std::process::id(),
                fastrand_u64(),
            ));
            let listener = UnixListener::bind(&path)?;
            let addr = path.to_string_lossy().into_owned();
            Ok((Self { listener, path }, addr))
        }

        pub(super) async fn accept(
            self,
        ) -> std::io::Result<std::pin::Pin<Box<dyn AsyncRead + Send>>> {
            let (stream, _addr) = self.listener.accept().await?;
            // We only read from the backend; drop the write half so the
            // backend sees a closed channel if it tries to read.
            let (read, _write) = stream.into_split();
            // The socket file is no longer needed once we have a connected
            // stream; clean it up now (the connection itself stays alive).
            let _ = std::fs::remove_file(&self.path);
            Ok(Box::pin(read))
        }
    }

    fn fastrand_u64() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}

#[cfg(windows)]
mod platform {
    use tokio::{
        io::AsyncRead,
        net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    };

    pub(super) struct PlatformListener {
        server: NamedPipeServer,
    }

    impl PlatformListener {
        pub(super) fn create() -> std::io::Result<(Self, String)> {
            let name = format!(
                r"\\.\pipe\pixi-build-log-{}-{:x}",
                std::process::id(),
                fastrand_u64(),
            );
            let server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)?;
            Ok((Self { server }, name))
        }

        pub(super) async fn accept(
            self,
        ) -> std::io::Result<std::pin::Pin<Box<dyn AsyncRead + Send>>> {
            self.server.connect().await?;
            Ok(Box::pin(self.server))
        }
    }

    fn fastrand_u64() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    }
}

use platform::PlatformListener;

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_build_types::log::{
        BackendEventRecord, BackendSpanCloseRecord, BackendSpanOpenRecord,
    };

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

    #[tokio::test]
    async fn end_to_end_listener_accepts_and_drains_records() {
        // Mimic the backend by spawning a writer task that connects to the
        // listener address as soon as it's available, then sends a couple
        // of records and disconnects. The pump should drain them without
        // panicking.
        let listener = LogListener::create().expect("create listener");
        let addr = listener.addr.clone();

        let writer = tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::io::AsyncWriteExt;
                use tokio::net::UnixStream;
                let mut attempts = 0;
                let mut stream = loop {
                    match UnixStream::connect(&addr).await {
                        Ok(s) => break s,
                        Err(_) if attempts < 20 => {
                            attempts += 1;
                            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                        }
                        Err(e) => panic!("connect: {e}"),
                    }
                };
                let payloads = [
                    r#"{"kind":"span_open","id":1,"level":"INFO","target":"t","name":"build"}"#,
                    r#"{"kind":"event","level":"WARN","target":"t","message":"hi","span_id":1}"#,
                    r#"{"kind":"span_close","id":1}"#,
                ];
                for p in payloads {
                    stream.write_all(p.as_bytes()).await.unwrap();
                    stream.write_all(b"\n").await.unwrap();
                }
                stream.shutdown().await.unwrap();
            }

            // Windows: keep this test as a no-op skeleton; the platform
            // listener API mirrors the same wire format so the pump_loop
            // path is exercised the same way once we add a winapi test
            // helper.
            #[cfg(not(unix))]
            {
                let _ = addr;
            }
        });

        let pump = listener.accept().await.expect("accept");
        // Give the pump a moment to drain the writer's lines before we
        // drop it. Aborting too early is fine for the test — the lines
        // would still parse — but waiting makes the intent obvious.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drop(pump);
        writer.await.unwrap();
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

/// Runtime-named `tracing` spans, used to mirror the backend's span hierarchy.
///
/// `tracing`'s public macros require span names, targets, and levels to be
/// `&'static` at the call site. We build the `Metadata`/`Callsite` pair by
/// hand, leaking one per unique `(target, name, level)` tuple. Keying on
/// level is important so envfilter rules don't filter out reconstructed
/// spans whose dyn callsite was created at the wrong level.
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
