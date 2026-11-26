//! Threat data attached to vulnerability matches: EPSS exploit-probability
//! scores, entries from CISA's Known Exploited Vulnerabilities catalog, and
//! the risk score derived from them.

use std::cmp::Ordering;

use serde::Serialize;
use threat_feeds::{EpssClient, EpssScore, KevClient, KevEntry, RansomwareCampaignUse};

use crate::report::SeverityRating;

/// Risk score in the range 0-100 combining how likely a vulnerability is
/// to be exploited with how severe it is, computed the way grype does so
/// the two tools rank vulnerability matches identically.
///
/// Threat is 1.0 for a KEV-listed vulnerability and the highest EPSS
/// probability among its CVEs otherwise. Severity averages the rating's midpoint
/// with the mean CVSS base score when one is known. KEV entries boost the
/// result by 5%, known ransomware use by 10%, and the product is capped at
/// 1.0 before scaling.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(transparent)]
pub struct RiskScore(f64);

impl RiskScore {
    /// No threat data and no severity contribution.
    pub const NONE: RiskScore = RiskScore(0.0);

    pub fn compute(
        severity: SeverityRating,
        cvss_base_score: Option<f64>,
        epss: &[EpssScore],
        kev: &[KevEntry],
    ) -> Self {
        let threat = if !kev.is_empty() {
            1.0
        } else {
            epss.iter()
                .map(|score| score.epss)
                .max_by(f64::total_cmp)
                .unwrap_or(0.0)
        };
        let rating_score = severity_midpoint(severity) / 10.0;
        let severity_term = match cvss_base_score {
            Some(base_score) if base_score > 0.0 => (rating_score + base_score / 10.0) / 2.0,
            Some(_) | None => rating_score,
        };
        let kev_modifier = if kev
            .iter()
            .any(|entry| entry.known_ransomware_campaign_use == RansomwareCampaignUse::Known)
        {
            1.1
        } else if !kev.is_empty() {
            1.05
        } else {
            1.0
        };
        RiskScore((threat * severity_term * kev_modifier).min(1.0) * 100.0)
    }

    pub fn value(self) -> f64 {
        self.0
    }
}

impl Default for RiskScore {
    fn default() -> Self {
        RiskScore::NONE
    }
}

impl Eq for RiskScore {}

impl PartialOrd for RiskScore {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RiskScore {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// The middle of each rating's CVSS range. Unknown deliberately lands in the
/// middle so unscored advisories are neither buried nor promoted.
fn severity_midpoint(severity: SeverityRating) -> f64 {
    match severity {
        SeverityRating::Critical => 9.0,
        SeverityRating::High => 7.5,
        SeverityRating::Medium => 5.0,
        SeverityRating::Low => 3.0,
        SeverityRating::Unknown => 5.0,
    }
}

/// Whether one threat-data source contributed to the report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ThreatSourceStatus {
    /// No client was configured for the source.
    #[default]
    Disabled,
    /// The source was fetched; `date` is the data's publication date when
    /// the source reports one.
    Loaded {
        #[serde(skip_serializing_if = "Option::is_none")]
        date: Option<String>,
    },
    /// The source could not be fetched. Vulnerability matches carry no data from it and
    /// the audit itself is unaffected.
    Failed { error: String },
}

/// Per-source availability of the threat data in a report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ThreatDataStatus {
    pub epss: ThreatSourceStatus,
    pub kev: ThreatSourceStatus,
}

/// The threat-data clients an audit may consult. Either source can be left
/// out, in which case vulnerability matches simply carry no data from it.
#[derive(Default, Clone, Copy)]
pub struct ThreatSources<'a> {
    pub epss: Option<&'a EpssClient>,
    pub kev: Option<&'a KevClient>,
}

#[cfg(test)]
mod tests;
