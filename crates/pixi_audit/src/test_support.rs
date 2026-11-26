pub use pixi_audit_test_support::spawn_router;
use reqwest_middleware::ClientWithMiddleware;
use url::Url;

use crate::{
    client::{BasiliskClient, OsvDevClient},
    report::PackageName,
};

fn http_client() -> ClientWithMiddleware {
    reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).build()
}

pub fn basilisk_client(base_url: Url) -> BasiliskClient {
    BasiliskClient::new(http_client(), base_url)
}

pub fn osv_client(base_url: Url) -> OsvDevClient {
    OsvDevClient::new(http_client(), base_url)
}

pub fn conda_name(name: &str) -> PackageName {
    PackageName::conda(name).unwrap()
}

pub fn pypi_name(name: &str) -> PackageName {
    PackageName::pypi(name).unwrap()
}
