use indicatif::MultiProgress;
use std::io;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone)]
pub struct IndicatifWriter {
    progress_bars: MultiProgress,
}

impl IndicatifWriter {
    pub fn new(pb: MultiProgress) -> Self {
        Self { progress_bars: pb }
    }
}

impl io::Write for IndicatifWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.progress_bars.suspend(|| io::stderr().write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.progress_bars.suspend(|| io::stderr().flush())
    }
}

impl<'a> MakeWriter<'a> for IndicatifWriter {
    type Writer = IndicatifWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
