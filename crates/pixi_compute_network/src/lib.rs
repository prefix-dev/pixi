//! Engine-side accessors for shared network infrastructure.
//!
//! Holds the cross-cutting `Has*` extension traits over network
//! resources that more than one domain Key needs (HTTP client,
//! eventually auth/proxy/rate-limit configuration). Domain crates
//! (e.g. `pixi_compute_sources`) depend on this crate rather than each
//! defining their own accessor.

use pixi_compute_engine::DataStore;
use rattler_networking::LazyClient;

/// Access the shared HTTP client used for network fetches (source URL
/// archives, conda binary package downloads).
pub trait HasDownloadClient {
    fn download_client(&self) -> &LazyClient;
}

impl HasDownloadClient for DataStore {
    fn download_client(&self) -> &LazyClient {
        self.get::<LazyClient>()
    }
}
