//! Audit orchestration across the Basilisk and PyPI databases, plus the
//! threat-data enrichment of the resulting vulnerability matches.

mod matches;

use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

use futures::{StreamExt, TryStreamExt};
use threat_feeds::{EpssClient, EpssScore, KevCatalog, KevClient};

use matches::MatchGroups;

use crate::{
    advisory::{
        BasiliskMatch, FixedVersions, basilisk_advisory_url, basilisk_match, evidence,
        fixed_versions, pypi_advisory_url, severities,
    },
    client::{AuditError, BasiliskClient, OsvDevClient},
    report::{
        AuditPackage, AuditReport, IgnoreMode, IgnoreRule, Match, MatchConfidence,
        PackageEcosystem, PackageId, SortKey, severity_summary,
    },
    threat::{RiskScore, ThreatDataStatus, ThreatSourceStatus, ThreatSources},
    types::{BatchResult, OsvQuery, OsvVulnerability, QueryPackage},
};

const MAX_CONCURRENT_DETAIL_FETCHES: usize = 16;

/// The vulnerability database a package is checked against, together with
/// the policy for interpreting what that database returns.
///
/// The discriminants index [`Database::ALL`] and the per-database tables in
/// [`query_databases`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Database {
    /// Basilisk, which answers for conda-forge packages.
    Basilisk = 0,
    /// OSV.dev, which answers for PyPI packages.
    Pypi = 1,
}

impl Database {
    const ALL: [Database; 2] = [Database::Basilisk, Database::Pypi];

    fn for_ecosystem(ecosystem: &PackageEcosystem) -> Option<Self> {
        match ecosystem {
            PackageEcosystem::CondaForge => Some(Database::Basilisk),
            PackageEcosystem::Pypi => Some(Database::Pypi),
            PackageEcosystem::Other(_) => None,
        }
    }

    fn summary(self, doc: &OsvVulnerability) -> Option<String> {
        match self {
            Database::Basilisk => doc.summary.clone(),
            // OSV.dev summaries are hard-wrapped; fold them onto one line.
            Database::Pypi => doc
                .summary
                .as_ref()
                .map(|summary| summary.split_whitespace().collect::<Vec<_>>().join(" ")),
        }
    }

    fn url(self, doc: &OsvVulnerability) -> Option<String> {
        Some(match self {
            Database::Basilisk => basilisk_advisory_url(doc),
            Database::Pypi => pypi_advisory_url(doc),
        })
    }

    fn fixed_versions(self, doc: &OsvVulnerability, package: &AuditPackage) -> FixedVersions {
        let mut fixes = fixed_versions(doc, &package.name, package.ecosystem.osv_ecosystem());
        if self == Database::Pypi {
            // Only PEP 440 versions are installable for a PyPI package, and
            // fixes outside PyPI are not reported for it.
            fixes
                .same_ecosystem
                .retain(|version| uv_pep440::Version::from_str(version).is_ok());
            fixes.other_ecosystem.clear();
            fixes.upstream_releases.clear();
        }
        fixes
    }

    /// How the database matched the advisory to the package. Only
    /// basilisk records this.
    fn basilisk_match(
        self,
        doc: &OsvVulnerability,
        package: &AuditPackage,
    ) -> Option<BasiliskMatch> {
        match self {
            Database::Basilisk => {
                basilisk_match(doc, &package.name, package.ecosystem.osv_ecosystem())
            }
            Database::Pypi => None,
        }
    }
}

struct Databases<'a> {
    basilisk: &'a BasiliskClient,
    pypi: &'a OsvDevClient,
}

impl Databases<'_> {
    fn client(&self, database: Database) -> &osv_client::OsvClient {
        match database {
            Database::Basilisk => self.basilisk.osv_client(),
            Database::Pypi => self.pypi.osv_client(),
        }
    }
}

/// A versioned package in a supported ecosystem, with its database query identity.
struct PlannedQuery<'a> {
    package: &'a AuditPackage,
    database: Database,
    /// The version as sent to the database.
    query_version: String,
    /// The package as locked, reported on vulnerability matches.
    id: PackageId,
}

impl PlannedQuery<'_> {
    fn osv_query(&self) -> OsvQuery {
        OsvQuery {
            package: QueryPackage {
                name: Some(self.package.name.to_string()),
                ecosystem: Some(self.package.ecosystem.osv_ecosystem().to_string()),
                purl: None,
            },
            version: Some(self.query_version.clone()),
            page_token: None,
        }
    }
}

/// Whether `package` can be checked against a vulnerability database.
pub(crate) fn is_auditable(package: &AuditPackage) -> bool {
    plan_query(package).is_some()
}

/// Versionless OSV queries match every affected version, so they are never sent.
fn plan_query(package: &AuditPackage) -> Option<PlannedQuery<'_>> {
    let version = package.version.clone()?;
    let database = Database::for_ecosystem(&package.ecosystem)?;
    let query_version = match database {
        Database::Basilisk => version.clone(),
        // Only PEP 440 versions are safe to send to OSV; a PyPI entry whose
        // version does not parse remains explicitly not audited.
        Database::Pypi => uv_pep440::Version::from_str(&version).ok()?.to_string(),
    };
    Some(PlannedQuery {
        package,
        database,
        query_version,
        id: PackageId {
            name: package.name.clone(),
            version,
            ecosystem: package.ecosystem.clone(),
        },
    })
}

/// Runs every planned query against its database, both databases in
/// parallel. The result at position `i` belongs to `planned[i]`.
async fn query_databases(
    databases: &Databases<'_>,
    planned: &[PlannedQuery<'_>],
) -> Result<Vec<BatchResult>, AuditError> {
    let mut positions: [Vec<usize>; Database::ALL.len()] = Default::default();
    let mut queries: [Vec<OsvQuery>; Database::ALL.len()] = Default::default();
    for (position, query) in planned.iter().enumerate() {
        positions[query.database as usize].push(position);
        queries[query.database as usize].push(query.osv_query());
    }
    let batches = futures::future::try_join_all(Database::ALL.iter().map(|&database| {
        databases
            .client(database)
            .query_batch(&queries[database as usize])
    }))
    .await?;

    let mut results = vec![BatchResult::default(); planned.len()];
    for (positions, batch) in positions.iter().zip(batches) {
        for (&position, result) in positions.iter().zip(batch) {
            results[position] = result;
        }
    }
    Ok(results)
}

/// Fetches each distinct referenced vulnerability document once, keyed by
/// database because the two databases have independent id spaces.
async fn fetch_documents(
    databases: &Databases<'_>,
    references: BTreeSet<(Database, String)>,
) -> Result<HashMap<(Database, String), OsvVulnerability>, AuditError> {
    futures::stream::iter(references)
        .map(|(database, id)| async move {
            let doc = databases.client(database).get_vuln(&id).await?;
            Ok::<_, AuditError>(((database, id), doc))
        })
        .buffer_unordered(MAX_CONCURRENT_DETAIL_FETCHES)
        .try_collect()
        .await
}

/// Audits `packages` against the vulnerability databases, applying the
/// `ignore` rules (vulnerability ids or aliases match case-insensitively;
/// permanent rules win over until-fixed ones), and attaches EPSS and KEV data
/// from `threat` to every vulnerability match.
///
/// A failed vulnerability lookup aborts the audit without a partial report.
/// Threat-feed failures only disable that source's ranking data and are recorded
/// in [`ThreatDataStatus`].
pub async fn audit(
    client: &BasiliskClient,
    osv_client: &OsvDevClient,
    packages: Vec<AuditPackage>,
    ignore: &[IgnoreRule],
    threat: ThreatSources<'_>,
) -> Result<AuditReport, AuditError> {
    let databases = Databases {
        basilisk: client,
        pypi: osv_client,
    };
    let mut not_audited = Vec::new();
    let mut planned = Vec::with_capacity(packages.len());
    for package in &packages {
        match plan_query(package) {
            Some(query) => planned.push(query),
            None => not_audited.push(package.clone()),
        }
    }
    // The KEV catalog does not depend on the vulnerability matches, so it downloads
    // while the vulnerability databases are queried.
    let (results, (kev_status, kev_catalog)) = futures::future::join(
        query_databases(&databases, &planned),
        load_kev_catalog(threat.kev),
    )
    .await;
    let results = results?;

    let references: BTreeSet<(Database, String)> = planned
        .iter()
        .zip(&results)
        .flat_map(|(query, result)| {
            result
                .vulns
                .iter()
                .map(move |vuln| (query.database, vuln.id.clone()))
        })
        .collect();
    let documents = fetch_documents(&databases, references).await?;
    let mut report = assemble_report(&planned, &results, &documents, ignore)?;
    report.not_audited = not_audited;

    let epss_status = attach_threat_data(&mut report, threat.epss, &kev_catalog).await;
    report.threat_data = ThreatDataStatus {
        epss: epss_status,
        kev: kev_status,
    };
    report.sort(SortKey::default());

    report.summary.not_audited = report.not_audited.len();
    Ok(report)
}

fn assemble_report(
    planned: &[PlannedQuery<'_>],
    results: &[BatchResult],
    documents: &HashMap<(Database, String), OsvVulnerability>,
    ignore: &[IgnoreRule],
) -> Result<AuditReport, AuditError> {
    let mut report = AuditReport::default();
    let mut ignores = Ignores::new(ignore);
    for (query, result) in planned.iter().zip(results) {
        let mut matches = MatchGroups::with_capacity(result.vulns.len());
        let package = query.package;
        for vuln in &result.vulns {
            let doc = documents
                .get(&(query.database, vuln.id.clone()))
                .ok_or_else(|| AuditError::MissingDocument {
                    id: vuln.id.clone(),
                })?;
            if doc.withdrawn.is_some() {
                continue;
            }
            let fixes = query.database.fixed_versions(doc, package);
            let basilisk_match = query.database.basilisk_match(doc, package);
            let (severity, cvss_base_score) = severity_summary(severities(
                doc,
                &package.name,
                package.ecosystem.osv_ecosystem(),
            ));
            let vuln_match = Match {
                package: query.id.clone(),
                environments: package.environments.clone(),
                id: doc.id.clone(),
                aliases: doc.aliases.clone(),
                severity,
                cvss_base_score,
                fixed_versions: fixes.same_ecosystem,
                advisories: vec![evidence(
                    doc,
                    &package.name,
                    package.ecosystem.osv_ecosystem(),
                )],
                other_ecosystem_fixes: fixes.other_ecosystem,
                upstream_releases: fixes.upstream_releases,
                match_confidence: basilisk_match
                    .as_ref()
                    .map_or(MatchConfidence::Unspecified, |m| m.match_confidence),
                matched_identity: basilisk_match.and_then(|m| m.matched_identity),
                summary: query.database.summary(doc),
                url: query.database.url(doc),
                epss: Vec::new(),
                kev: Vec::new(),
                risk: RiskScore::NONE,
                ignore_mode: None,
            };
            matches.push(vuln_match);
        }
        for vuln_match in matches.into_matches() {
            record_match(&mut report, vuln_match, &mut ignores);
        }
    }
    report.unused_ignores = ignores.unmatched();
    report.summary.audited = planned.len();
    report.summary.vulnerable = report.matches.len();
    report.summary.ignored = report.ignored.len();
    Ok(report)
}

async fn load_kev_catalog(client: Option<&KevClient>) -> (ThreatSourceStatus, KevCatalog) {
    let Some(client) = client else {
        return (ThreatSourceStatus::Disabled, KevCatalog::default());
    };
    match client.catalog().await {
        Ok(catalog) => {
            let date = catalog.date_released().map(str::to_string);
            (ThreatSourceStatus::Loaded { date }, catalog)
        }
        Err(err) => (
            ThreatSourceStatus::Failed {
                error: error_chain(&err),
            },
            KevCatalog::default(),
        ),
    }
}

/// Looks up EPSS scores for every CVE across the report's vulnerability matches, attaches
/// them together with the KEV entries from `kev`, and computes each
/// vulnerability match's risk. Returns the EPSS source's status.
async fn attach_threat_data(
    report: &mut AuditReport,
    epss_client: Option<&EpssClient>,
    kev: &KevCatalog,
) -> ThreatSourceStatus {
    let cves: Vec<String> = report
        .matches
        .iter()
        .chain(&report.ignored)
        .flat_map(Match::cve_ids)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let (epss_status, epss_scores): (ThreatSourceStatus, HashMap<String, EpssScore>) =
        match epss_client {
            None => (ThreatSourceStatus::Disabled, HashMap::new()),
            Some(client) => match client.scores(&cves).await {
                Ok(scores) => {
                    let date = scores.first().map(|score| score.date.clone());
                    let by_cve = scores
                        .into_iter()
                        .map(|score| (score.cve.to_ascii_uppercase(), score))
                        .collect();
                    (ThreatSourceStatus::Loaded { date }, by_cve)
                }
                Err(err) => (
                    ThreatSourceStatus::Failed {
                        error: error_chain(&err),
                    },
                    HashMap::new(),
                ),
            },
        };

    for vuln_match in report.matches.iter_mut().chain(report.ignored.iter_mut()) {
        let cves = vuln_match.cve_ids();
        vuln_match.epss = cves
            .iter()
            .filter_map(|cve| epss_scores.get(cve).cloned())
            .collect();
        vuln_match.kev = cves
            .iter()
            .filter_map(|cve| kev.get(cve).cloned())
            .collect();
        vuln_match.risk = RiskScore::compute(
            vuln_match.severity,
            vuln_match.cvss_base_score,
            &vuln_match.epss,
            &vuln_match.kev,
        );
    }
    epss_status
}

/// The error and every cause below it, joined for a one-line status.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

/// A single rule list and an indexed record of every rule that matched.
struct Ignores<'a> {
    rules: &'a [IgnoreRule],
    needs_locked_version: bool,
    /// Matching is recorded even when another rule wins or a fix is known.
    matched: Vec<bool>,
}

impl<'a> Ignores<'a> {
    fn new(rules: &'a [IgnoreRule]) -> Self {
        Self {
            rules,
            needs_locked_version: rules.iter().any(IgnoreRule::needs_locked_version),
            matched: vec![false; rules.len()],
        }
    }

    /// Why `vuln_match` is ignored, if it is. Unconditional rules win over
    /// until-fixed ones, which lapse once a fixed release newer than the
    /// locked version is reported in the package's own ecosystem.
    fn reason_for(&mut self, vuln_match: &Match) -> Option<IgnoreMode> {
        let mut permanent = false;
        let mut until_fixed = false;
        // Parse once per vulnerability match, and only when a rule has version bounds.
        let locked = if self.needs_locked_version {
            vuln_match.package.parsed_version().ok()
        } else {
            None
        };
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.applies_to(vuln_match, locked.as_ref()) {
                self.matched[index] = true;
                match rule.mode() {
                    IgnoreMode::UntilFixed => until_fixed = true,
                    IgnoreMode::Permanent => permanent = true,
                }
            }
        }
        let fixed_in_ecosystem = vuln_match.has_fix_after_locked_version();
        if permanent {
            Some(IgnoreMode::Permanent)
        } else if until_fixed && !fixed_in_ecosystem {
            Some(IgnoreMode::UntilFixed)
        } else {
            None
        }
    }

    /// Unmatched rules in their original spelling and order. Unscoped ids
    /// are deduplicated case-insensitively across both ignore modes.
    fn unmatched(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.rules
            .iter()
            .zip(&self.matched)
            .filter(|(rule, matched)| {
                !**matched
                    && (!rule.scope().is_empty()
                        || rule.id().is_none_or(|id| seen.insert(id.to_lowercase())))
            })
            .map(|(rule, _)| rule.to_string())
            .collect()
    }
}

fn record_match(report: &mut AuditReport, mut vuln_match: Match, ignores: &mut Ignores) {
    match ignores.reason_for(&vuln_match) {
        Some(reason) => {
            vuln_match.ignore_mode = Some(reason);
            report.ignored.push(vuln_match);
        }
        None => report.matches.push(vuln_match),
    }
}

#[cfg(test)]
mod tests;
