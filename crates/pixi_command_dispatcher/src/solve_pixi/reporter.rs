use url::Url;

pub struct WrappingGatewayReporter(pub Box<dyn rattler_repodata_gateway::Reporter>);

impl rattler_repodata_gateway::Reporter for WrappingGatewayReporter {
    fn on_download_start(&self, url: &Url) -> usize {
        self.0.on_download_start(url)
    }
    fn on_download_progress(
        &self,
        url: &Url,
        index: usize,
        bytes_downloaded: usize,
        total_bytes: Option<usize>,
    ) {
        self.0
            .on_download_progress(url, index, bytes_downloaded, total_bytes)
    }
    fn on_download_complete(&self, url: &Url, index: usize) {
        self.0.on_download_complete(url, index)
    }
    fn on_jlap_start(&self) -> usize {
        self.0.on_jlap_start()
    }
    fn on_jlap_decode_start(&self, index: usize) {
        self.0.on_jlap_decode_start(index)
    }
    fn on_jlap_decode_completed(&self, index: usize) {
        self.0.on_jlap_decode_completed(index)
    }
    fn on_jlap_apply_patch(&self, index: usize, patch_index: usize, total: usize) {
        self.0.on_jlap_apply_patch(index, patch_index, total)
    }
    fn on_jlap_apply_patches_completed(&self, index: usize) {
        self.0.on_jlap_apply_patches_completed(index)
    }
    fn on_jlap_encode_start(&self, index: usize) {
        self.0.on_jlap_encode_start(index)
    }
    fn on_jlap_encode_completed(&self, index: usize) {
        self.0.on_jlap_encode_completed(index)
    }
    fn on_jlap_completed(&self, index: usize) {
        self.0.on_jlap_completed(index)
    }
}
