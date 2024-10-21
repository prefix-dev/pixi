use std::{collections::VecDeque, sync::Arc, time::Instant};

use parking_lot::Mutex;
use url::Url;

use super::SolveProgressBar;

pub struct GatewayProgressReporter {
    inner: Mutex<InnerProgressState>,
}

impl GatewayProgressReporter {
    pub(crate) fn new(pb: Arc<SolveProgressBar>) -> Self {
        Self {
            inner: Mutex::new(InnerProgressState {
                pb,
                downloads: VecDeque::new(),

                bytes_downloaded: 0,
                total_bytes: 0,
                total_pending_downloads: 0,

                jlap: VecDeque::default(),
                total_pending_jlap: 0,
            }),
        }
    }
}

struct InnerProgressState {
    pb: Arc<SolveProgressBar>,

    downloads: VecDeque<DownloadState>,

    bytes_downloaded: usize,
    total_bytes: usize,
    total_pending_downloads: usize,

    jlap: VecDeque<JLAPState>,
    total_pending_jlap: usize,
}

impl InnerProgressState {
    fn update_progress(&self) {
        if self.total_pending_downloads > 0 {
            self.pb.set_bytes_update_style(self.total_bytes);
            self.pb.set_position(self.bytes_downloaded as u64);
            self.pb.set_message("downloading repodata");
        } else if self.total_pending_jlap > 0 {
            self.pb.reset_style();
            self.pb.set_message("applying JLAP patches");
        } else {
            self.pb.reset_style();
            self.pb.set_message("parsing repodata");
        }
    }
}

struct DownloadState {
    _started_at: Instant,
    bytes_downloaded: usize,
    total_size: usize,
    _finished_at: Option<Instant>,
}

struct JLAPState {
    _started_at: Instant,
    _finished_at: Option<Instant>,
}

impl rattler_repodata_gateway::Reporter for GatewayProgressReporter {
    fn on_download_start(&self, _url: &Url) -> usize {
        let mut inner = self.inner.lock();
        let download_idx = inner.downloads.len();
        inner.downloads.push_back(DownloadState {
            _started_at: Instant::now(),
            bytes_downloaded: 0,
            total_size: 0,
            _finished_at: None,
        });
        inner.total_pending_downloads += 1;
        inner.update_progress();
        download_idx
    }

    fn on_download_progress(
        &self,
        _url: &Url,
        index: usize,
        bytes_downloaded: usize,
        total_bytes: Option<usize>,
    ) {
        let mut inner = self.inner.lock();

        let download = inner
            .downloads
            .get_mut(index)
            .expect("download index should exist");

        let prev_bytes_downloaded = download.bytes_downloaded;
        let prev_total_size = download.total_size;
        download.bytes_downloaded = bytes_downloaded;
        download.total_size = total_bytes.unwrap_or(0);

        inner.bytes_downloaded = inner.bytes_downloaded + bytes_downloaded - prev_bytes_downloaded;
        inner.total_bytes = inner.total_bytes + total_bytes.unwrap_or(0) - prev_total_size;

        inner.update_progress();
    }

    fn on_download_complete(&self, _url: &Url, _index: usize) {
        let mut inner = self.inner.lock();
        let download = inner
            .downloads
            .get_mut(_index)
            .expect("download index should exist");
        download._finished_at = Some(Instant::now());

        inner.total_pending_downloads -= 1;

        inner.update_progress();
    }

    fn on_jlap_start(&self) -> usize {
        let mut inner = self.inner.lock();

        let index = inner.jlap.len();
        inner.jlap.push_back(JLAPState {
            _started_at: Instant::now(),
            _finished_at: None,
        });
        inner.total_pending_jlap += 1;

        inner.update_progress();

        index
    }

    fn on_jlap_completed(&self, index: usize) {
        let mut inner = self.inner.lock();
        let jlap = inner.jlap.get_mut(index).expect("jlap index should exist");
        jlap._finished_at = Some(Instant::now());
        inner.total_pending_jlap -= 1;

        inner.update_progress();
    }
}
