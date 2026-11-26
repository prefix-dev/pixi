//! Audit locked packages for known vulnerabilities against OSV-compatible
//! vulnerability APIs: Basilisk for conda-forge and OSV.dev for PyPI.
//!
//! The crate does not discover workspaces or write files. Callers configure
//! the HTTP clients and decide what to do with reports and remediation plans.

mod advisory;
mod audit;
pub mod client;
mod cvss_v2;
pub mod lock;
pub mod policy;
pub mod remediation;
pub mod report;
#[cfg(test)]
mod test_support;
pub mod threat;

pub use audit::audit;
pub use client::{
    AuditError, BASILISK_URL_ENV_VAR, BasiliskClient, DEFAULT_BASILISK_URL, DEFAULT_OSV_URL,
    OSV_URL_ENV_VAR, OsvDevClient, TOKEN_ENV_VAR, TOKEN_LENGTH, TOKEN_PREFIX,
    looks_like_basilisk_token,
};
pub use lock::{
    Coverage, CoverageKey, CoverageRegression, CoverageStatus, LockFileAuditError, audit_lock_file,
    collect_packages,
};
pub use osv_protocol as types;
pub use policy::{Policy, Verdict};
pub use remediation::{ManifestRequirement, PlanBlocker, RemediationPlan, TemporaryConstraint};
pub use report::{
    AdvisoryEvidence, AffectedRange, AuditPackage, AuditReport, AuditSummary, CondaForgeBuild,
    FixAvailability, IgnoreMode, IgnoreRule, IgnoreScope, InvalidIgnoreRule, InvalidIgnoreScope,
    InvalidPackageName, Match, MatchConfidence, MatchedIdentity, MatchedIdentityKind,
    OtherEcosystemFix, PackageEcosystem, PackageId, PackageName, PackageVersion,
    ParsePackageVersionError, ParseSeverityRatingError, RangeEvent, RangeSource, SeverityRating,
    SortKey, UpstreamRelease,
};
pub use threat::{RiskScore, ThreatDataStatus, ThreatSourceStatus, ThreatSources};
pub use threat_feeds::{
    DEFAULT_EPSS_BASE_URL, DEFAULT_KEV_URL, EPSS_BASE_URL_ENV_VAR, EpssClient, EpssScore,
    KEV_URL_ENV_VAR, KevCatalog, KevClient, KevEntry, RansomwareCampaignUse, ThreatFeedError,
};
