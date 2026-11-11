use std::{
    io::Write,
    sync::{Arc, Mutex},
};

use tracing::subscriber::DefaultGuard;
use tracing_subscriber::{
    filter::LevelFilter,
    fmt::{self, writer::MakeWriter},
};

/// A mock writer that can be used to capture logs in tests.
#[derive(Clone)]
pub struct MockWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl MockWriter {
    /// Creates a new `MockWriter`.
    pub fn new() -> Self {
        Self {
            buf: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Returns the captured output as a string.
    pub fn get_output(&self) -> String {
        let buf = self.buf.lock().unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }
}

impl Write for MockWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.buf.lock().unwrap().flush()
    }
}

impl<'a> MakeWriter<'a> for MockWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// A scoped tracing subscriber that captures warning logs for one test.
pub struct TestSubscriber {
    writer: MockWriter,
    _guard: DefaultGuard,
}

impl TestSubscriber {
    /// Returns the captured output as a string.
    pub fn get_output(&self) -> String {
        self.writer.get_output()
    }
}

/// Initializes a scoped tracing subscriber for tests.
///
/// This uses a thread-local default subscriber instead of the global default so
/// it still captures logs if another integration test already initialized
/// tracing globally.
pub fn try_init_test_subscriber() -> TestSubscriber {
    let writer = MockWriter::new();
    let subscriber = fmt::Subscriber::builder()
        .with_max_level(LevelFilter::WARN)
        .with_writer(writer.clone())
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);

    TestSubscriber {
        writer,
        _guard: guard,
    }
}
