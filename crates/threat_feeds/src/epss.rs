use reqwest_middleware::ClientWithMiddleware;
use serde::{Deserialize, Deserializer};
use url::Url;

use super::{ThreatFeedError, http};
use crate::EpssScore;

/// The default EPSS API instance (FIRST).
pub const DEFAULT_EPSS_BASE_URL: &str = "https://api.first.org";

/// Environment variable to override the EPSS API base URL.
pub const EPSS_BASE_URL_ENV_VAR: &str = "PIXI_AUDIT_EPSS_BASE_URL";

/// The API returns at most this many records per response, so requests
/// carry at most this many CVEs and never need paging.
const MAX_CVES_PER_REQUEST: usize = 100;

/// Client for the FIRST EPSS API.
pub struct EpssClient {
    client: ClientWithMiddleware,
    base_url: Url,
}

impl EpssClient {
    pub fn new(client: ClientWithMiddleware, base_url: Url) -> Self {
        Self {
            client,
            base_url: http::base_url(base_url),
        }
    }

    /// Fetches the current EPSS record for each of `cves`, sorted by CVE id.
    /// CVEs the API has no score for are absent from the result.
    pub async fn scores(&self, cves: &[String]) -> Result<Vec<EpssScore>, ThreatFeedError> {
        let mut scores = Vec::new();
        for chunk in cves.chunks(MAX_CVES_PER_REQUEST) {
            let mut url = self
                .base_url
                .join("data/v1/epss")
                .map_err(ThreatFeedError::InvalidBaseUrl)?;
            url.query_pairs_mut()
                .append_pair("cve", &chunk.join(","))
                .append_pair("limit", &MAX_CVES_PER_REQUEST.to_string());
            let body: EpssResponse = http::send_json(self.client.get(url.clone()))
                .await
                .map_err(|failure| ThreatFeedError::from_http(url, failure))?;
            scores.extend(body.data.into_iter().map(|record| EpssScore {
                cve: record.cve,
                epss: record.epss,
                percentile: record.percentile,
                date: record.date,
            }));
        }
        scores.sort_by(|a, b| a.cve.cmp(&b.cve));
        Ok(scores)
    }
}

#[derive(Deserialize)]
struct EpssResponse {
    data: Vec<EpssRecord>,
}

/// One record of the API's `data` array. Scores are decimal strings such
/// as `"0.907700000"`.
#[derive(Deserialize)]
struct EpssRecord {
    cve: String,
    #[serde(deserialize_with = "deserialize_decimal")]
    epss: f64,
    #[serde(deserialize_with = "deserialize_decimal")]
    percentile: f64,
    date: String,
}

fn deserialize_decimal<'de, D: Deserializer<'de>>(deserializer: D) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Decimal {
        Number(f64),
        Text(String),
    }
    match Decimal::deserialize(deserializer)? {
        Decimal::Number(value) => Ok(value),
        Decimal::Text(text) => text.trim().parse().map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests;
