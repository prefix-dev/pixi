use std::sync::Mutex;

#[derive(Default)]
pub struct CacheMetrics {
    data: Mutex<CacheMetricsData>,
}

impl CacheMetrics {
    pub fn record_request_response(&self, response: &reqwest::Response) {
        let cache_header = response.headers().get("x-cache");
        if cache_header.and_then(|h| h.to_str().ok()) == Some("HIT") {
            let mut data = self.data.lock().unwrap();
            data.cache_hits += 1;
        } else {
            let mut data = self.data.lock().unwrap();
            data.cache_misses += 1;
            tracing::debug!("Cache miss on '{}' ({})", response.url(), response.status());
        }
    }

    pub(crate) fn into_data(self) -> CacheMetricsData {
        self.data
            .into_inner()
            .expect("locking shouldnt fail in this case")
    }
}

#[derive(Default)]
pub(crate) struct CacheMetricsData {
    pub cache_hits: usize,
    pub cache_misses: usize,
}
