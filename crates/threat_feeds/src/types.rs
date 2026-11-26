use serde::{Deserialize, Serialize};

/// One EPSS record for a CVE, as published by FIRST.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EpssScore {
    pub cve: String,
    /// Probability that the CVE is exploited in the next 30 days, 0.0-1.0.
    pub epss: f64,
    /// Rank of `epss` among all scored CVEs, 0.0-1.0.
    pub percentile: f64,
    /// Score date, `YYYY-MM-DD`.
    pub date: String,
}

/// Whether a KEV catalog entry is known to be used in ransomware campaigns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RansomwareCampaignUse {
    Known,
    Unknown,
}

impl RansomwareCampaignUse {
    /// Parses the catalog's `knownRansomwareCampaignUse` value; anything
    /// other than `Known` counts as unknown.
    pub fn parse(value: &str) -> Self {
        if value.trim().eq_ignore_ascii_case("known") {
            RansomwareCampaignUse::Known
        } else {
            RansomwareCampaignUse::Unknown
        }
    }
}

/// An entry of CISA's Known Exploited Vulnerabilities catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KevEntry {
    pub cve: String,
    pub vendor_project: String,
    pub product: String,
    pub vulnerability_name: String,
    /// `YYYY-MM-DD`.
    pub date_added: String,
    pub required_action: String,
    /// `YYYY-MM-DD`.
    pub due_date: String,
    pub known_ransomware_campaign_use: RansomwareCampaignUse,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

#[cfg(test)]
mod tests;
