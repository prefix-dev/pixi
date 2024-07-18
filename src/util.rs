use indicatif::MultiProgress;
use rattler_conda_types::ChannelConfig;
use std::io;
use tracing_subscriber::fmt::MakeWriter;
use url::Url;

#[derive(Clone)]
pub struct IndicatifWriter {
    progress_bars: MultiProgress,
}

impl IndicatifWriter {
    pub(crate) fn new(pb: MultiProgress) -> Self {
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

pub fn default_channel_config() -> ChannelConfig {
    ChannelConfig::default_with_root_dir(
        std::env::current_dir().expect("Could not retrieve the current directory"),
    )
}

/// If the URL points to a subdirectory, extract it, as in (git):
///   `git+https://git.example.com/MyProject.git@v1.0#subdirectory=pkg_dir`
///   `git+https://git.example.com/MyProject.git@v1.0#egg=pkg&subdirectory=pkg_dir`
/// or (direct archive url):
///   `https://github.com/foo-labs/foo/archive/master.zip#subdirectory=packages/bar`
///   `https://github.com/foo-labs/foo/archive/master.zip#egg=pkg&subdirectory=packages/bar`
pub fn extract_directory_from_url(url: &Url) -> Option<String> {
    let fragment = url.fragment()?;
    let subdirectory = fragment
        .split('&')
        .find_map(|fragment| fragment.strip_prefix("subdirectory="))?;
    Some(subdirectory.into())
}
