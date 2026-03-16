use rattler_repodata_gateway::DownloadReporter;

pub struct WrappingGatewayReporter(pub Box<dyn rattler_repodata_gateway::Reporter>);

impl rattler_repodata_gateway::Reporter for WrappingGatewayReporter {
    fn download_reporter(&self) -> Option<&dyn DownloadReporter> {
        self.0.download_reporter()
    }
}
