//! OSV advisory interpretation, including Basilisk fix metadata.

use serde::Deserialize;

use crate::{
    report::{
        AdvisoryEvidence, AffectedRange, CondaForgeBuild, MatchConfidence, MatchedIdentity,
        MatchedIdentityKind, OtherEcosystemFix, PackageName, RangeEvent, RangeSource,
        UpstreamRelease,
    },
    types::{OsvAffected, OsvSeverity, OsvVulnerability},
};

#[derive(Debug, Deserialize)]
struct BasiliskFixMetadata {
    fix_version: String,
    conda_publication: CondaForgeBuild,
}

/// The `basilisk` block of an affected entry's `database_specific`.
fn basilisk_block(database_specific: Option<&serde_json::Value>) -> Option<&serde_json::Value> {
    database_specific?.get("basilisk")
}

fn basilisk_fix_metadata(block: &serde_json::Value) -> Option<BasiliskFixMetadata> {
    let metadata: BasiliskFixMetadata = serde_json::from_value(block.clone()).ok()?;
    (!metadata.fix_version.trim().is_empty()).then_some(metadata)
}

/// How basilisk matched an advisory to the queried conda-forge package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BasiliskMatch {
    pub match_confidence: MatchConfidence,
    pub matched_identity: Option<MatchedIdentity>,
}

impl BasiliskMatch {
    /// Reads one match's fields. Each field is read on its own, so a value
    /// this version does not understand leaves that field unspecified
    /// instead of discarding the whole block.
    fn from_value(value: &serde_json::Value) -> Self {
        let field = |name: &str| value.get(name).cloned();
        Self {
            match_confidence: field("applicability")
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or(MatchConfidence::Unspecified),
            matched_identity: field("matched_identity")
                .and_then(|value| serde_json::from_value(value).ok()),
        }
    }

    /// Keep applicability and identity from the same match. A purl on a
    /// weaker match cannot establish the provenance of a stronger CPE match.
    /// For equal applicability, prefer purl, then CPE, then no identity.
    fn from_block(block: &serde_json::Value) -> Self {
        let Some(matches) = block.get("matches").and_then(|m| m.as_array()) else {
            return Self::from_value(block);
        };
        matches
            .iter()
            .map(Self::from_value)
            .max_by_key(|m| {
                (
                    m.match_confidence,
                    m.matched_identity.is_some(),
                    m.matched_identity
                        .as_ref()
                        .is_some_and(|identity| identity.kind == MatchedIdentityKind::Purl),
                )
            })
            .unwrap_or_else(|| Self::from_value(block))
    }
}

/// Whether `affected` is a conda-forge entry that explicitly names the
/// queried package, which is where basilisk puts its metadata.
fn is_explicit_conda_forge_match(
    affected: &OsvAffected,
    package: &PackageName,
    ecosystem: &str,
) -> bool {
    ecosystem.eq_ignore_ascii_case("conda-forge")
        && affected.package.as_ref().is_some_and(|entry| {
            entry
                .name
                .as_deref()
                .is_some_and(|name| package.matches(name))
                && entry
                    .ecosystem
                    .as_deref()
                    .is_some_and(|entry| entry.eq_ignore_ascii_case(ecosystem))
        })
}

/// Basilisk's match metadata for `package`, from the first conda-forge
/// entry that names it and carries a `basilisk` block. `None` for other
/// ecosystems and for plain OSV documents.
pub(crate) fn basilisk_match(
    doc: &OsvVulnerability,
    package: &PackageName,
    ecosystem: &str,
) -> Option<BasiliskMatch> {
    doc.affected
        .iter()
        .filter(|affected| is_explicit_conda_forge_match(affected, package, ecosystem))
        .find_map(|affected| basilisk_block(affected.database_specific.as_ref()))
        .map(BasiliskMatch::from_block)
}

/// Which entries in other ecosystems may contribute other-ecosystem fixes.
#[derive(Debug, PartialEq, Eq)]
enum CrossEcosystemRule {
    /// Any entry naming the same package.
    ByName,
    /// Only entries whose purl is this one, as [`purl_key`] compares them.
    ByPurl(String),
    /// None: the match says nothing about other ecosystems.
    Nothing,
}

impl CrossEcosystemRule {
    /// A purl identity vouches for exactly that package elsewhere; a CPE
    /// vouches for nothing outside conda-forge. Without an identity the
    /// name is trusted only when basilisk evaluated the match, or when the
    /// document is plain OSV with no basilisk metadata at all.
    fn for_match(basilisk_match: Option<&BasiliskMatch>) -> Self {
        let Some(basilisk_match) = basilisk_match else {
            return Self::ByName;
        };
        match &basilisk_match.matched_identity {
            Some(MatchedIdentity {
                kind: MatchedIdentityKind::Purl,
                value,
            }) => Self::ByPurl(purl_key(value)),
            Some(MatchedIdentity {
                kind: MatchedIdentityKind::Cpe,
                ..
            }) => Self::Nothing,
            None if basilisk_match.match_confidence == MatchConfidence::Evaluated => Self::ByName,
            None => Self::Nothing,
        }
    }

    fn accepts(&self, affected: &OsvAffected, package: &PackageName) -> bool {
        let entry = affected.package.as_ref();
        match self {
            Self::ByName => entry
                .and_then(|entry| entry.name.as_deref())
                .is_none_or(|name| package.matches(name)),
            Self::ByPurl(key) => entry
                .and_then(|entry| entry.purl.as_deref())
                .is_some_and(|purl| purl_key(purl) == *key),
            Self::Nothing => false,
        }
    }
}

/// The part of a purl that identifies a package: `pkg:<type>/<namespace>/
/// <name>`, without the version, qualifiers, or subpath, case-folded.
fn purl_key(purl: &str) -> String {
    let purl = purl.split(['?', '#']).next().unwrap_or_default();
    let name_start = purl.rfind('/').map_or(0, |slash| slash + 1);
    let end = purl[name_start..]
        .find('@')
        .map_or(purl.len(), |at| name_start + at);
    purl[..end]
        .trim_end_matches('/')
        .replace("%40", "@")
        .to_lowercase()
}

fn matching_affected<'a>(
    doc: &'a OsvVulnerability,
    package: &'a PackageName,
    ecosystem: &'a str,
    rule: CrossEcosystemRule,
) -> impl Iterator<Item = (&'a OsvAffected, Option<&'a str>)> {
    doc.affected.iter().filter_map(move |affected| {
        let entry_name = affected.package.as_ref().and_then(|p| p.name.as_deref());
        let other_ecosystem = affected
            .package
            .as_ref()
            .and_then(|p| p.ecosystem.as_deref())
            .filter(|entry| !entry.eq_ignore_ascii_case(ecosystem));
        let accepted = match other_ecosystem {
            // An entry naming no package applies to the queried one.
            None => entry_name.is_none_or(|name| package.matches(name)),
            Some(_) => rule.accepts(affected, package),
        };
        accepted.then_some((affected, other_ecosystem))
    })
}

/// Document-wide severity plus severity belonging to the matched package.
pub(crate) fn severities<'a>(
    doc: &'a OsvVulnerability,
    package: &'a PackageName,
    ecosystem: &'a str,
) -> impl Iterator<Item = &'a OsvSeverity> {
    let rule = basilisk_match(doc, package, ecosystem)
        .map_or(CrossEcosystemRule::Nothing, |matched| {
            CrossEcosystemRule::for_match(Some(&matched))
        });
    doc.severity.iter().chain(
        matching_affected(doc, package, ecosystem, rule)
            .flat_map(|(affected, _)| affected.severity.iter()),
    )
}

/// Fixed versions for one package from an OSV document, split by origin.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FixedVersions {
    /// Fixes released in the queried ecosystem.
    pub same_ecosystem: Vec<String>,
    /// Fixed releases of the same package in other ecosystems. Informative
    /// only: those releases may not exist in the queried ecosystem yet.
    pub other_ecosystem: Vec<OtherEcosystemFix>,
    /// Fixes the project released upstream without a conda-forge build.
    pub upstream_releases: Vec<UpstreamRelease>,
}

impl FixedVersions {
    fn push_same_ecosystem(&mut self, version: &str) {
        if !self
            .same_ecosystem
            .iter()
            .any(|existing| existing == version)
        {
            self.same_ecosystem.push(version.to_string());
        }
    }

    /// Whether a fix outside the queried ecosystem already lists `version`.
    fn has_external(&self, version: &str) -> bool {
        self.other_ecosystem
            .iter()
            .any(|fix| fix.version == version)
            || self
                .upstream_releases
                .iter()
                .any(|release| release.version == version)
    }

    /// The first fix reported for a version wins.
    fn push_other_ecosystem(&mut self, version: &str, ecosystem: &str) {
        if !self.has_external(version) {
            self.other_ecosystem.push(OtherEcosystemFix {
                version: version.to_string(),
                ecosystem: ecosystem.to_string(),
            });
        }
    }

    /// The first fix reported for a version wins.
    fn push_upstream_release(&mut self, version: &str, conda_forge_build: CondaForgeBuild) {
        if !self.has_external(version) {
            self.upstream_releases.push(UpstreamRelease {
                version: version.to_string(),
                conda_forge_build,
            });
        }
    }
}

/// `fixed` events from the entry's release ranges (`ECOSYSTEM`/`SEMVER`).
/// Git ranges carry commit hashes, which are not installable versions.
fn release_fixed_events(affected: &OsvAffected) -> impl Iterator<Item = &str> {
    affected
        .ranges
        .iter()
        .filter(|range| {
            range.kind.as_deref().is_some_and(|kind| {
                kind.eq_ignore_ascii_case("ECOSYSTEM") || kind.eq_ignore_ascii_case("SEMVER")
            })
        })
        .flat_map(|range| range.events.iter())
        .filter_map(|event| event.fixed.as_deref())
}

/// Fixed versions for `package` from an OSV document, split by origin.
///
/// Entries in the queried ecosystem, or naming no package, contribute
/// same-ecosystem fixes. Names use the ecosystem's normalization rules.
///
/// Cross-ecosystem fixes follow [`CrossEcosystemRule`]. Basilisk metadata applies
/// only to conda-forge entries explicitly naming the queried package.
pub(crate) fn fixed_versions(
    doc: &OsvVulnerability,
    package: &PackageName,
    ecosystem: &str,
) -> FixedVersions {
    let rule = CrossEcosystemRule::for_match(basilisk_match(doc, package, ecosystem).as_ref());
    let mut fixes = FixedVersions::default();
    for (affected, other_ecosystem) in matching_affected(doc, package, ecosystem, rule) {
        for fixed in release_fixed_events(affected) {
            match other_ecosystem {
                None => fixes.push_same_ecosystem(fixed),
                Some(other) => fixes.push_other_ecosystem(fixed, other),
            }
        }

        if is_explicit_conda_forge_match(affected, package, ecosystem)
            && let Some(metadata) =
                basilisk_block(affected.database_specific.as_ref()).and_then(basilisk_fix_metadata)
        {
            // A CPE-matched fix is the project's own release; only a
            // conda-forge build makes it a fixed version here.
            match metadata.conda_publication {
                CondaForgeBuild::Available => fixes.push_same_ecosystem(&metadata.fix_version),
                conda_forge_build => {
                    fixes.push_upstream_release(&metadata.fix_version, conda_forge_build)
                }
            }
        }
    }
    // A version already reported as fixed in the package's own ecosystem is
    // not additionally a fix elsewhere.
    let same_ecosystem = &fixes.same_ecosystem;
    fixes
        .other_ecosystem
        .retain(|fix| !same_ecosystem.contains(&fix.version));
    fixes
        .upstream_releases
        .retain(|release| !same_ecosystem.contains(&release.version));
    fixes
}

/// What `doc` says about `package` in its own ecosystem: the release ranges
/// and listed versions of every same-ecosystem entry, plus basilisk's fix
/// metadata when conda-forge has a build of the fix. Git ranges carry commit
/// hashes, and fixes elsewhere are not installable, so neither contributes.
pub(crate) fn evidence(
    doc: &OsvVulnerability,
    package: &PackageName,
    ecosystem: &str,
) -> AdvisoryEvidence {
    let mut evidence = AdvisoryEvidence {
        id: doc.id.clone(),
        ranges: Vec::new(),
        versions: Vec::new(),
    };
    let rule = CrossEcosystemRule::for_match(basilisk_match(doc, package, ecosystem).as_ref());
    for (affected, other_ecosystem) in matching_affected(doc, package, ecosystem, rule) {
        if other_ecosystem.is_some() {
            continue;
        }
        for range in &affected.ranges {
            let source = match range.kind.as_deref() {
                Some(kind) if kind.eq_ignore_ascii_case("ECOSYSTEM") => RangeSource::Ecosystem,
                Some(kind) if kind.eq_ignore_ascii_case("SEMVER") => RangeSource::Semver,
                _ => continue,
            };
            let events = range
                .events
                .iter()
                .flat_map(|event| {
                    [
                        event.introduced.clone().map(RangeEvent::Introduced),
                        event.fixed.clone().map(RangeEvent::Fixed),
                        event.last_affected.clone().map(RangeEvent::LastAffected),
                    ]
                })
                .flatten()
                .collect();
            evidence.ranges.push(AffectedRange { source, events });
        }
        evidence.versions.extend(affected.versions.iter().cloned());
        if is_explicit_conda_forge_match(affected, package, ecosystem)
            && let Some(metadata) =
                basilisk_block(affected.database_specific.as_ref()).and_then(basilisk_fix_metadata)
            && metadata.conda_publication == CondaForgeBuild::Available
        {
            evidence.ranges.push(AffectedRange {
                source: RangeSource::BasiliskFix,
                events: vec![
                    RangeEvent::Introduced("0".to_string()),
                    RangeEvent::Fixed(metadata.fix_version),
                ],
            });
        }
    }
    evidence
}

fn reference_url<'a>(doc: &'a OsvVulnerability, kind: &str) -> Option<&'a str> {
    doc.references
        .iter()
        .find(|reference| reference.kind.as_deref() == Some(kind))
        .map(|reference| reference.url.as_str())
}

/// The Basilisk explorer page for this advisory.
pub(crate) fn basilisk_advisory_url(doc: &OsvVulnerability) -> String {
    format!("https://basilisk.prefix.dev/advisories/{}", doc.id)
}

/// The advisory link for an OSV.dev document: the `ADVISORY` reference,
/// else the `WEB` reference, else the document's own osv.dev page.
pub(crate) fn pypi_advisory_url(doc: &OsvVulnerability) -> String {
    reference_url(doc, "ADVISORY")
        .or_else(|| reference_url(doc, "WEB"))
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://osv.dev/vulnerability/{}", doc.id))
}

#[cfg(test)]
mod tests;
