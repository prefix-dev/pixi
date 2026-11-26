//! Audit inputs and report types.

use std::cmp::Ordering;
use std::str::FromStr;

use serde::{Deserialize, Serialize, Serializer};
use thiserror::Error;

use threat_feeds::{EpssScore, KevEntry};

use crate::{
    threat::{RiskScore, ThreatDataStatus},
    types::OsvSeverity,
};

/// Where a locked package comes from, for vulnerability-query purposes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PackageEcosystem {
    /// A conda package from the conda-forge channel. The only ecosystem
    /// basilisk's query API answers for today.
    CondaForge,
    /// A PyPI package.
    Pypi,
    /// A conda package from another channel (e.g. bioconda), or a source
    /// package. The contained string is a human-readable origin.
    Other(String),
}

impl PackageEcosystem {
    /// Classifies a conda channel URL: conda-forge is the only ecosystem the
    /// vulnerability database can currently answer for; anything else is
    /// `Other` named after the channel. A label (`<channel>/label/<name>`)
    /// publishes builds of the same packages under the same versions, so it
    /// belongs to its channel's ecosystem.
    pub fn from_channel_url(channel_url: Option<&str>) -> Self {
        let Some(url) = channel_url else {
            return PackageEcosystem::Other("unknown".to_string());
        };
        let trimmed = url.trim_end_matches('/');
        let channel = trimmed
            .rsplit_once("/label/")
            .map_or(trimmed, |(channel, _label)| channel);
        if matches!(
            channel,
            "https://conda.anaconda.org/conda-forge" | "https://prefix.dev/conda-forge"
        ) {
            PackageEcosystem::CondaForge
        } else {
            let name = channel.rsplit('/').next().unwrap_or("unknown");
            PackageEcosystem::Other(name.to_string())
        }
    }

    /// The ecosystem string sent in OSV queries.
    pub fn osv_ecosystem(&self) -> &str {
        match self {
            PackageEcosystem::CondaForge => "conda-forge",
            PackageEcosystem::Pypi => "PyPI",
            PackageEcosystem::Other(origin) => origin,
        }
    }
}

/// Displays as the OSV ecosystem string (`conda-forge`, `PyPI`, or the
/// channel name). JSON output serializes the same string.
impl std::fmt::Display for PackageEcosystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.osv_ecosystem())
    }
}

impl Serialize for PackageEcosystem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A parsed version carrying its ecosystem's comparison rules.
///
/// Versions from different ecosystems are incomparable. No total ordering is
/// defined: callers must handle `partial_cmp` returning `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageVersion {
    Conda(rattler_conda_types::Version),
    Pypi(uv_pep440::Version),
}

/// Distinguishes malformed versions from ecosystems without comparison support.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParsePackageVersionError {
    #[error("'{version}' is not a valid {ecosystem} version")]
    InvalidVersion {
        ecosystem: PackageEcosystem,
        version: String,
    },
    #[error("version comparison is not supported for ecosystem '{0}'")]
    UnsupportedEcosystem(PackageEcosystem),
}

impl PackageVersion {
    /// Parses using the selected ecosystem, without falling back to another
    /// version format. Unsupported ecosystems return an explicit error.
    pub fn parse(
        ecosystem: &PackageEcosystem,
        value: &str,
    ) -> Result<Self, ParsePackageVersionError> {
        let invalid = || ParsePackageVersionError::InvalidVersion {
            ecosystem: ecosystem.clone(),
            version: value.to_string(),
        };
        match ecosystem {
            PackageEcosystem::CondaForge => value.parse().map(Self::Conda).map_err(|_| invalid()),
            PackageEcosystem::Pypi => value.parse().map(Self::Pypi).map_err(|_| invalid()),
            PackageEcosystem::Other(_) => Err(ParsePackageVersionError::UnsupportedEcosystem(
                ecosystem.clone(),
            )),
        }
    }
}

impl PartialOrd for PackageVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Conda(a), Self::Conda(b)) => Some(a.cmp(b)),
            (Self::Pypi(a), Self::Pypi(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }
}

/// A package name that carries its ecosystem's normalization rules, so two
/// spellings compare equal exactly when the ecosystem says they do. Conda
/// names are lowercased; PyPI names follow PEP 503, under which
/// `Ruamel.YAML`, `ruamel_yaml`, and `ruamel-yaml` are one package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PackageName {
    Conda(rattler_conda_types::PackageName),
    Pypi(uv_normalize::PackageName),
}

#[derive(Debug, Clone, Error)]
pub enum InvalidPackageName {
    #[error("'{name}' is not a valid conda package name")]
    Conda {
        name: String,
        #[source]
        source: rattler_conda_types::InvalidPackageNameError,
    },
    #[error("'{name}' is not a valid PyPI package name")]
    Pypi {
        name: String,
        #[source]
        source: uv_normalize::InvalidNameError,
    },
}

impl PackageName {
    /// Parses a conda package name, lowercasing it.
    pub fn conda(name: &str) -> Result<Self, InvalidPackageName> {
        rattler_conda_types::PackageName::from_str(name)
            .map(Self::Conda)
            .map_err(|source| InvalidPackageName::Conda {
                name: name.to_string(),
                source,
            })
    }

    /// Parses a PyPI package name, normalizing it per PEP 503.
    pub fn pypi(name: &str) -> Result<Self, InvalidPackageName> {
        uv_normalize::PackageName::from_str(name)
            .map(Self::Pypi)
            .map_err(|source| InvalidPackageName::Pypi {
                name: name.to_string(),
                source,
            })
    }

    /// Parses `name` with the rules of `ecosystem`: PyPI names follow
    /// PEP 503, every other ecosystem is a conda channel.
    pub fn parse(ecosystem: &PackageEcosystem, name: &str) -> Result<Self, InvalidPackageName> {
        match ecosystem {
            PackageEcosystem::Pypi => Self::pypi(name),
            PackageEcosystem::CondaForge | PackageEcosystem::Other(_) => Self::conda(name),
        }
    }

    /// The normalized name, as sent in queries and written to reports.
    pub fn as_str(&self) -> &str {
        match self {
            PackageName::Conda(name) => name.as_normalized(),
            PackageName::Pypi(name) => name.as_str(),
        }
    }

    /// Whether `spelling`, as written in an advisory or a manifest, names
    /// this package under the ecosystem's normalization rules.
    pub fn matches(&self, spelling: &str) -> bool {
        match self {
            PackageName::Conda(name) => name.as_normalized().eq_ignore_ascii_case(spelling),
            PackageName::Pypi(name) => {
                uv_normalize::PackageName::from_str(spelling).is_ok_and(|other| other == *name)
            }
        }
    }
}

/// Displays as the normalized name. JSON output serializes the same string.
impl std::fmt::Display for PackageName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for PackageName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// A deduplicated locked package to audit. Reports list the packages that
/// could not be checked in this form too.
#[derive(Debug, Clone, Serialize)]
pub struct AuditPackage {
    #[serde(rename = "package")]
    pub name: PackageName,
    /// `None` when the version could not be determined (e.g. an unresolved
    /// conda source package). Such packages are never queried — a
    /// version-less OSV query matches every version ever affected — and are
    /// always reported as not audited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub ecosystem: PackageEcosystem,
    /// Environments (across all platforms) that contain this exact package.
    pub environments: Vec<String>,
}

/// Identifies one locked package with a known version: the same name at a
/// different version or from a different ecosystem is a different package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PackageId {
    #[serde(rename = "package")]
    pub name: PackageName,
    pub version: String,
    pub ecosystem: PackageEcosystem,
}

impl PackageId {
    /// The locked version, parsed with the package's ecosystem rules.
    pub fn parsed_version(&self) -> Result<PackageVersion, ParsePackageVersionError> {
        PackageVersion::parse(&self.ecosystem, &self.version)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SeverityRating {
    Unknown,
    Low,
    Medium,
    High,
    Critical,
}

impl SeverityRating {
    /// Every rating, least severe first.
    pub const ALL: [SeverityRating; 5] = [
        SeverityRating::Unknown,
        SeverityRating::Low,
        SeverityRating::Medium,
        SeverityRating::High,
        SeverityRating::Critical,
    ];

    /// The rating's lowercase name, as used in JSON output and on the command
    /// line.
    pub fn name(self) -> &'static str {
        match self {
            SeverityRating::Critical => "critical",
            SeverityRating::High => "high",
            SeverityRating::Medium => "medium",
            SeverityRating::Low => "low",
            SeverityRating::Unknown => "unknown",
        }
    }

    /// The highest CVSS rating across the document's severity entries.
    /// Understands the type labels basilisk emits: `CVSS_V3`/`CVSS_V3.0`/
    /// `CVSS_V3.1` (OSV and NVD sources), `CVSS_V4`, and `CVSS_V2.0`.
    /// Ratings follow basilisk: critical >= 9.0, high >= 7.0, medium >= 4.0.
    pub fn from_severities(severities: &[OsvSeverity]) -> Self {
        severity_summary(severities).0
    }

    fn from_score(score: f64) -> Self {
        match score {
            s if s >= 9.0 => SeverityRating::Critical,
            s if s >= 7.0 => SeverityRating::High,
            s if s >= 4.0 => SeverityRating::Medium,
            _ => SeverityRating::Low,
        }
    }
}

enum CvssVersion {
    V2,
    V3,
    V4,
}

/// The CVSS version and base score of one severity entry, when its vector
/// parses. Understands the type labels basilisk emits: `CVSS_V3`/
/// `CVSS_V3.0`/`CVSS_V3.1` (OSV and NVD sources), `CVSS_V4`, and `CVSS_V2.0`.
fn parse_base_score(severity: &OsvSeverity) -> Option<(CvssVersion, f64)> {
    if severity.kind.starts_with("CVSS_V3") {
        let base = severity.score.parse::<cvss::v3::Base>().ok()?;
        Some((CvssVersion::V3, base.score().value()))
    } else if severity.kind.starts_with("CVSS_V4") {
        let vector = severity.score.parse::<cvss::v4::Vector>().ok()?;
        Some((CvssVersion::V4, vector.score().value()))
    } else if severity.kind.starts_with("CVSS_V2") {
        let score = crate::cvss_v2::base_score(&severity.score)?;
        Some((CvssVersion::V2, score))
    } else {
        None
    }
}

/// Mean positive CVSS base score, or `None` if no positive score parses.
pub fn mean_cvss_base_score(severities: &[OsvSeverity]) -> Option<f64> {
    severity_summary(severities).1
}

pub(crate) fn severity_summary<'a>(
    severities: impl IntoIterator<Item = &'a OsvSeverity>,
) -> (SeverityRating, Option<f64>) {
    let mut rating = SeverityRating::Unknown;
    let mut total = 0.0;
    let mut count = 0;
    for (version, score) in severities.into_iter().filter_map(parse_base_score) {
        let entry_rating = match version {
            CvssVersion::V3 | CvssVersion::V4 => SeverityRating::from_score(score),
            // CVSS v2's qualitative scale has no critical rating.
            CvssVersion::V2 => SeverityRating::from_score(score).min(SeverityRating::High),
        };
        rating = rating.max(entry_rating);
        if score > 0.0 {
            total += score;
            count += 1;
        }
    }
    (rating, (count > 0).then(|| total / f64::from(count)))
}

impl std::fmt::Display for SeverityRating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("'{0}' is not a severity rating")]
pub struct ParseSeverityRatingError(String);

/// Parses the exact lowercase rating names produced by [`SeverityRating::name`].
impl FromStr for SeverityRating {
    type Err = ParseSeverityRatingError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|rating| rating.name() == s)
            .ok_or_else(|| ParseSeverityRatingError(s.to_string()))
    }
}

/// How long an ignore rule applies, and so why a vulnerability match sits in
/// the ignored section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IgnoreMode {
    /// The rule applies unconditionally.
    Permanent,
    /// The rule applies while no fixed release is reported in the package's
    /// own ecosystem. A fix reported only upstream or in another ecosystem
    /// does not end it.
    UntilFixed,
}

/// Package, ecosystem and installed-version predicates shared by ignore
/// rules, as written in the configuration.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoreScope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ecosystem: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(rename = "up-to-version", skip_serializing_if = "Option::is_none")]
    pub up_to_version: Option<String>,
}

impl IgnoreScope {
    pub fn is_empty(&self) -> bool {
        self.package.is_none()
            && self.ecosystem.is_none()
            && self.version.is_none()
            && self.up_to_version.is_none()
    }

    /// Checks that the ecosystem is supported and every version bound
    /// parses for it.
    pub fn validate(&self) -> Result<(), InvalidIgnoreScope> {
        ScopeMatcher::parse(self).map(drop)
    }
}

/// Why an [`IgnoreScope`] cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidIgnoreScope {
    #[error("unsupported ecosystem '{0}' (expected conda-forge or PyPI)")]
    UnsupportedEcosystem(String),
    #[error("invalid version '{0}' for the selected ecosystem")]
    InvalidVersion(String),
}

/// An [`IgnoreScope`] parsed for matching: the ecosystem resolved and every
/// version bound parsed once.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ScopeMatcher {
    package: Option<String>,
    ecosystem: Option<PackageEcosystem>,
    version: Option<VersionBound>,
    up_to_version: Option<VersionBound>,
}

impl ScopeMatcher {
    fn parse(scope: &IgnoreScope) -> Result<Self, InvalidIgnoreScope> {
        let ecosystem = scope
            .ecosystem
            .as_deref()
            .map(|ecosystem| {
                if ecosystem.eq_ignore_ascii_case("conda-forge") {
                    Ok(PackageEcosystem::CondaForge)
                } else if ecosystem.eq_ignore_ascii_case("pypi") {
                    Ok(PackageEcosystem::Pypi)
                } else {
                    Err(InvalidIgnoreScope::UnsupportedEcosystem(
                        ecosystem.to_ascii_lowercase(),
                    ))
                }
            })
            .transpose()?;
        let bound = |version: &Option<String>| {
            version
                .as_deref()
                .map(|version| VersionBound::parse(ecosystem.as_ref(), version))
                .transpose()
        };
        Ok(Self {
            package: scope.package.clone(),
            version: bound(&scope.version)?,
            up_to_version: bound(&scope.up_to_version)?,
            ecosystem,
        })
    }

    /// Whether `package`, whose locked version parsed as `locked`, falls
    /// under this scope. A version-bounded scope never matches a package
    /// whose version does not parse (`locked` is `None`).
    fn matches(&self, package: &PackageId, locked: Option<&PackageVersion>) -> bool {
        let other_package = self
            .package
            .as_deref()
            .is_some_and(|name| !package.name.matches(name));
        let other_ecosystem = self
            .ecosystem
            .as_ref()
            .is_some_and(|ecosystem| *ecosystem != package.ecosystem);
        if other_package || other_ecosystem {
            return false;
        }
        if self.version.is_none() && self.up_to_version.is_none() {
            return true;
        }
        let Some(locked) = locked else {
            return false;
        };
        self.version
            .as_ref()
            .is_none_or(|bound| bound.compare(locked) == Some(Ordering::Equal))
            && self.up_to_version.as_ref().is_none_or(|bound| {
                matches!(
                    bound.compare(locked),
                    Some(Ordering::Less | Ordering::Equal)
                )
            })
    }
}

/// A version bound of an ignore rule, parsed with each version scheme the
/// rule applies to. A rule without an ecosystem keeps both parses, so every
/// package is compared under its own ecosystem's rules.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VersionBound {
    conda: Option<rattler_conda_types::Version>,
    pypi: Option<uv_pep440::Version>,
}

impl VersionBound {
    fn parse(ecosystem: Option<&PackageEcosystem>, text: &str) -> Result<Self, InvalidIgnoreScope> {
        let bound = match ecosystem {
            Some(PackageEcosystem::CondaForge) => Self {
                conda: text.parse().ok(),
                pypi: None,
            },
            Some(PackageEcosystem::Pypi) => Self {
                conda: None,
                pypi: text.parse().ok(),
            },
            // Unqualified; a scope never names another ecosystem.
            Some(PackageEcosystem::Other(_)) | None => Self {
                conda: text.parse().ok(),
                pypi: text.parse().ok(),
            },
        };
        if bound.conda.is_none() && bound.pypi.is_none() {
            return Err(InvalidIgnoreScope::InvalidVersion(text.to_string()));
        }
        Ok(bound)
    }

    /// How `locked` compares with the bound, or `None` when the bound has
    /// no parse in `locked`'s ecosystem.
    fn compare(&self, locked: &PackageVersion) -> Option<Ordering> {
        match locked {
            PackageVersion::Conda(version) => self.conda.as_ref().map(|bound| version.cmp(bound)),
            PackageVersion::Pypi(version) => self.pypi.as_ref().map(|bound| version.cmp(bound)),
        }
    }
}

/// Why an ignore rule cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidIgnoreRule {
    /// A rule naming neither an advisory nor a package would ignore
    /// everything.
    #[error("an ignore rule needs an advisory id or a package")]
    MissingTarget,
    /// Until-fixed exceptions are per advisory; a package-wide one would be a
    /// blanket exception for every unfixed advisory.
    #[error("an until-fixed ignore rule needs an advisory id")]
    UntilFixedWithoutId,
    #[error(transparent)]
    Scope(#[from] InvalidIgnoreScope),
}

/// An ignore rule optionally scoped to package, ecosystem and locked
/// version. Its scope is parsed on construction, so matching never
/// re-parses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreRule {
    id: Option<String>,
    scope: IgnoreScope,
    mode: IgnoreMode,
    matcher: ScopeMatcher,
}

impl IgnoreRule {
    pub fn new(
        id: Option<String>,
        scope: IgnoreScope,
        mode: IgnoreMode,
    ) -> Result<Self, InvalidIgnoreRule> {
        match (&id, &scope.package, mode) {
            (None, None, _) => return Err(InvalidIgnoreRule::MissingTarget),
            (None, Some(_), IgnoreMode::UntilFixed) => {
                return Err(InvalidIgnoreRule::UntilFixedWithoutId);
            }
            _ => {}
        }
        let matcher = ScopeMatcher::parse(&scope)?;
        Ok(Self {
            id,
            scope,
            mode,
            matcher,
        })
    }

    /// An unscoped vulnerability id or alias, preserved as written.
    pub fn from_id(id: String, mode: IgnoreMode) -> Self {
        Self {
            id: Some(id),
            scope: IgnoreScope::default(),
            mode,
            matcher: ScopeMatcher::default(),
        }
    }

    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The scope as written.
    pub fn scope(&self) -> &IgnoreScope {
        &self.scope
    }

    /// How long the rule applies.
    pub fn mode(&self) -> IgnoreMode {
        self.mode
    }

    pub(crate) fn needs_locked_version(&self) -> bool {
        self.matcher.version.is_some() || self.matcher.up_to_version.is_some()
    }

    /// Whether the rule ignores `vuln_match`, whose locked version parsed
    /// as `locked`.
    pub(crate) fn applies_to(&self, vuln_match: &Match, locked: Option<&PackageVersion>) -> bool {
        self.id.as_ref().is_none_or(|id| {
            std::iter::once(&vuln_match.id)
                .chain(&vuln_match.aliases)
                .any(|candidate| candidate.eq_ignore_ascii_case(id))
        }) && self.matcher.matches(&vuln_match.package, locked)
    }
}

/// The rule as written, e.g. `CVE-1 in package 'openssl', version '3.1.0'`.
impl std::fmt::Display for IgnoreRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scope = &self.scope;
        let parts = [
            (None, self.id.as_deref()),
            (Some("package"), scope.package.as_deref()),
            (Some("ecosystem"), scope.ecosystem.as_deref()),
            (Some("version"), scope.version.as_deref()),
            (Some("up to version"), scope.up_to_version.as_deref()),
        ];
        let mut separator = "";
        for (label, value) in parts {
            let Some(value) = value else {
                continue;
            };
            f.write_str(separator)?;
            match label {
                Some(label) => write!(f, "{label} '{value}'")?,
                None => f.write_str(value)?,
            }
            separator = if label.is_none() { " in " } else { ", " };
        }
        Ok(())
    }
}

/// One advisory record's statement about the audited package's own
/// ecosystem: its affected ranges and listed affected versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdvisoryEvidence {
    /// The id of the document the record belongs to.
    pub id: String,
    /// Affected ranges, each with its events in record order.
    pub ranges: Vec<AffectedRange>,
    /// Versions the record lists as affected.
    pub versions: Vec<String>,
}

/// One affected range of an advisory record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AffectedRange {
    pub source: RangeSource,
    pub events: Vec<RangeEvent>,
}

/// Where an affected range comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RangeSource {
    /// An OSV `ECOSYSTEM` range.
    Ecosystem,
    /// An OSV `SEMVER` range.
    Semver,
    /// Basilisk's fix metadata for a release with a conda-forge build: every
    /// version before the fix is affected.
    BasiliskFix,
}

/// A boundary of an affected range, in the package's ecosystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RangeEvent {
    Introduced(String),
    Fixed(String),
    LastAffected(String),
}

/// Whether conda-forge has a build of a fixed release, as basilisk reports
/// it. Deserialized from basilisk's `conda_publication` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all(serialize = "kebab-case", deserialize = "snake_case"))]
pub enum CondaForgeBuild {
    #[serde(rename(deserialize = "published"))]
    Available,
    #[serde(rename(deserialize = "not_published"))]
    Missing,
    Unknown,
}

/// How the advisory's version data matched the audited conda-forge package.
/// Orders matches by specificity so the maximum is the strongest one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all(serialize = "kebab-case", deserialize = "snake_case"))]
pub enum MatchConfidence {
    /// Basilisk recorded no applicability: PyPI vulnerability matches, and documents
    /// whose conda-forge entry carries no basilisk metadata.
    #[serde(skip_deserializing)]
    Unspecified,
    /// The advisory names the vendor and product (a CPE match) but gives
    /// no version bounds basilisk could apply, so every version matches.
    VersionUnbounded,
    /// The package matched but the affected versions were not evaluated.
    Unevaluated,
    /// The installed version matches the advisory's affected version data.
    Evaluated,
}

/// The kind of identifier an advisory was matched to the package through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MatchedIdentityKind {
    /// A package URL, such as `pkg:npm/pnpm`: the advisory is about that
    /// exact package in that ecosystem.
    Purl,
    /// A CPE name from NVD: a vendor and product, with no ecosystem.
    Cpe,
}

/// The identifier basilisk matched an advisory to the conda-forge package
/// through. Fixes in other ecosystems are only trusted when they belong to
/// this identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedIdentity {
    pub kind: MatchedIdentityKind,
    pub value: String,
}

/// A fixed release of the same package in another ecosystem, such as the
/// PyPI release of a package audited from conda-forge. Informative only: it
/// may not be installable for the audited package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OtherEcosystemFix {
    pub version: String,
    pub ecosystem: String,
}

/// A fix the project itself released, known from advisories matched by
/// CPE, where no ecosystem is involved. `conda_forge_build` is never
/// `Available` here, since that counts as a fixed version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpstreamRelease {
    pub version: String,
    pub conda_forge_build: CondaForgeBuild,
}

/// One (package, vulnerability) pair.
#[derive(Debug, Clone, Serialize)]
pub struct Match {
    /// Serialized as the top-level `package`, `version`, and `ecosystem`.
    #[serde(flatten)]
    pub package: PackageId,
    pub environments: Vec<String>,
    pub id: String,
    pub aliases: Vec<String>,
    pub severity: SeverityRating,
    /// Mean CVSS base score across the advisory's parsable severity entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cvss_base_score: Option<f64>,
    /// Fixed versions in the package's own ecosystem, across every
    /// contributing advisory.
    pub fixed_versions: Vec<String>,
    /// What each contributing advisory record says about the package's own
    /// ecosystem, kept per record so remediation never mixes the boundaries
    /// of different records.
    pub advisories: Vec<AdvisoryEvidence>,
    /// Fixed releases of the same package in other ecosystems.
    /// Informative only; remediation cannot install these.
    pub other_ecosystem_fixes: Vec<OtherEcosystemFix>,
    /// Fixes the project released upstream without a conda-forge build.
    /// Informative only; remediation cannot install these.
    pub upstream_releases: Vec<UpstreamRelease>,
    /// How confidently basilisk matched the advisory to the package.
    /// `Unspecified` for PyPI vulnerability matches and documents without metadata.
    pub match_confidence: MatchConfidence,
    /// The identifier basilisk matched the advisory through, when known.
    pub matched_identity: Option<MatchedIdentity>,
    pub summary: Option<String>,
    pub url: Option<String>,
    /// EPSS records for the vulnerability match's CVE ids, sorted by CVE.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub epss: Vec<EpssScore>,
    /// KEV catalog entries for the vulnerability match's CVE ids.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kev: Vec<KevEntry>,
    pub risk: RiskScore,
    /// Set only on vulnerability matches in the ignored section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ignore_mode: Option<IgnoreMode>,
}

impl Match {
    /// The vulnerability match's CVE ids: its own id and any CVE alias, deduplicated
    /// case-insensitively and upper-cased.
    pub fn cve_ids(&self) -> Vec<String> {
        let mut cves: Vec<String> = std::iter::once(&self.id)
            .chain(&self.aliases)
            .filter(|id| id.len() > 4 && id.as_bytes()[..4].eq_ignore_ascii_case(b"CVE-"))
            .map(|id| id.to_ascii_uppercase())
            .collect();
        cves.sort();
        cves.dedup();
        cves
    }

    /// Whether a fixed release newer than the locked version is reported in
    /// the package's own ecosystem. A fix of an older release branch does
    /// not fix the locked version. When versions cannot be compared, any
    /// reported fix counts.
    pub fn has_fix_after_locked_version(&self) -> bool {
        let Ok(locked) = self.package.parsed_version() else {
            return !self.fixed_versions.is_empty();
        };
        self.fixed_versions.iter().any(|fixed| {
            PackageVersion::parse(&self.package.ecosystem, fixed)
                .map_or(true, |fixed| fixed > locked)
        })
    }

    /// Where the vulnerability match's fix stands.
    pub fn fix_availability(&self) -> FixAvailability {
        if !self.fixed_versions.is_empty() {
            FixAvailability::Available
        } else if !self.other_ecosystem_fixes.is_empty() || !self.upstream_releases.is_empty() {
            FixAvailability::ElsewhereOnly
        } else {
            FixAvailability::None
        }
    }
}

/// Where a vulnerability match's fix stands, from [`Match::fix_availability`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixAvailability {
    /// A fixed release exists in the package's own ecosystem.
    Available,
    /// Fixes exist only outside the package's own ecosystem, so they may
    /// not be installable for it.
    ElsewhereOnly,
    /// No fix is known anywhere.
    None,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditSummary {
    /// Packages in an ecosystem the database can answer for.
    pub audited: usize,
    pub vulnerable: usize,
    pub ignored: usize,
    pub not_audited: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AuditReport {
    pub matches: Vec<Match>,
    pub ignored: Vec<Match>,
    pub not_audited: Vec<AuditPackage>,
    pub summary: AuditSummary,
    /// Ignore ids and scoped rules that matched no vulnerability. An expired
    /// package or version exception remains visible after an upgrade.
    pub unused_ignores: Vec<String>,
    pub threat_data: ThreatDataStatus,
}

impl AuditReport {
    /// Orders the vulnerable and ignored vulnerability matches by `key`.
    pub fn sort(&mut self, key: SortKey) {
        self.matches.sort_by(|a, b| key.compare(a, b));
        self.ignored.sort_by(|a, b| key.compare(a, b));
    }
}

/// How vulnerability matches are ordered in a report. Every key is a total order:
/// after its primary key it falls through the remaining keys so equal
/// vulnerability matches keep a stable, predictable order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// Highest [`RiskScore`] first.
    #[default]
    Risk,
    /// Highest [`SeverityRating`] first.
    Severity,
    /// Highest EPSS percentile first.
    Epss,
    /// KEV-listed vulnerability matches first.
    Kev,
    /// Package name, version, and ecosystem.
    Package,
    /// Vulnerability id.
    Vulnerability,
}

impl SortKey {
    pub const ALL: [SortKey; 6] = [
        SortKey::Risk,
        SortKey::Severity,
        SortKey::Epss,
        SortKey::Kev,
        SortKey::Package,
        SortKey::Vulnerability,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SortKey::Risk => "risk",
            SortKey::Severity => "severity",
            SortKey::Epss => "epss",
            SortKey::Kev => "kev",
            SortKey::Package => "package",
            SortKey::Vulnerability => "vulnerability",
        }
    }

    pub fn compare(self, a: &Match, b: &Match) -> Ordering {
        type Key = fn(&Match, &Match) -> Ordering;
        let keys: &[Key] = match self {
            SortKey::Risk => &[by_risk, by_severity, by_epss_percentile, by_package, by_id],
            SortKey::Severity => &[by_severity, by_risk, by_epss_percentile, by_package, by_id],
            SortKey::Epss => &[by_epss_percentile, by_risk, by_severity, by_package, by_id],
            SortKey::Kev => &[
                by_kev,
                by_risk,
                by_severity,
                by_epss_percentile,
                by_package,
                by_id,
            ],
            SortKey::Package => &[by_package, by_id, by_risk, by_severity, by_epss_percentile],
            SortKey::Vulnerability => {
                &[by_id, by_risk, by_severity, by_epss_percentile, by_package]
            }
        };
        keys.iter()
            .map(|key| key(a, b))
            .find(|ordering| ordering.is_ne())
            .unwrap_or(Ordering::Equal)
    }
}

impl std::fmt::Display for SortKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for SortKey {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        SortKey::ALL
            .into_iter()
            .find(|key| key.name() == name)
            .ok_or_else(|| {
                let options: Vec<&str> = SortKey::ALL.iter().map(|s| s.name()).collect();
                format!(
                    "'{name}' is not a sort key (expected one of {})",
                    options.join(", ")
                )
            })
    }
}

fn by_risk(a: &Match, b: &Match) -> Ordering {
    b.risk.cmp(&a.risk)
}

fn by_severity(a: &Match, b: &Match) -> Ordering {
    b.severity.cmp(&a.severity)
}

fn by_epss_percentile(a: &Match, b: &Match) -> Ordering {
    let percentile = |vuln_match: &Match| {
        vuln_match
            .epss
            .iter()
            .max_by(|left, right| left.epss.total_cmp(&right.epss))
            .map(|score| score.percentile)
            .unwrap_or(0.0)
    };
    percentile(b).total_cmp(&percentile(a))
}

fn by_kev(a: &Match, b: &Match) -> Ordering {
    b.kev.len().cmp(&a.kev.len())
}

/// By spelling, not by [`PackageName`]'s ecosystem-first derive.
fn by_package(a: &Match, b: &Match) -> Ordering {
    let (a, b) = (&a.package, &b.package);
    a.name
        .as_str()
        .cmp(b.name.as_str())
        .then_with(|| a.version.cmp(&b.version))
        .then_with(|| a.ecosystem.cmp(&b.ecosystem))
}

fn by_id(a: &Match, b: &Match) -> Ordering {
    a.id.cmp(&b.id)
}

#[cfg(test)]
mod tests;
