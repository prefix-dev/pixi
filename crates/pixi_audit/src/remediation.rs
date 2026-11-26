//! Remediation planning: temporary constraints that move every
//! policy-failing package out of the versions its advisories affect.
//!
//! Each advisory record is interpreted on its own with the package
//! ecosystem's version rules, so the boundary of one record never stands in
//! for another. The versions no contributing record affects form the safe
//! set. Only the parts of it at or after a documented boundary newer than
//! the locked version are used: a `fixed` release, or the version after a
//! `last_affected` one. Versions the advisories merely do not mention, and
//! fixes of older release branches, are not remediation. A documented fix
//! is still not proof that later releases are safe, so the candidate that
//! the solver produces must be audited again.

mod ranges;

use std::{collections::BTreeMap, ops::Bound, str::FromStr};

use rattler_conda_types::{
    ParseStrictness, Version as CondaVersion, VersionSpec,
    version_spec::{EqualityOperator, LogicalOperator, RangeOperator},
};
use thiserror::Error;
use uv_pep440::{Operator, Version as PypiVersion, VersionSpecifier, VersionSpecifiers};

use ranges::{Component, Interval, IntervalSet};

use crate::{
    policy::Policy,
    report::{AdvisoryEvidence, AuditReport, Match, MatchConfidence, PackageName, RangeEvent},
};

/// A version requirement the workspace places on a package, as written in
/// the manifest. Remediation constraints must leave room for it, because
/// they are added to it rather than replacing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRequirement {
    pub package: PackageName,
    pub spec: String,
}

/// A temporary constraint that excludes the versions the advisories of a
/// package affect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporaryConstraint {
    Conda {
        name: rattler_conda_types::PackageName,
        version: VersionSpec,
        advisories: Vec<String>,
    },
    Pypi {
        name: uv_normalize::PackageName,
        specifiers: VersionSpecifiers,
        advisories: Vec<String>,
    },
}

impl std::fmt::Display for TemporaryConstraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemporaryConstraint::Conda { name, version, .. } => {
                write!(f, "{} {version}", name.as_normalized())
            }
            TemporaryConstraint::Pypi {
                name, specifiers, ..
            } => write!(f, "{name}{specifiers}"),
        }
    }
}

/// Why no constraint can resolve a policy-failing vulnerability match.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanBlocker {
    #[error("{package} {version} has no fixed release for {advisory} in its own ecosystem")]
    NoFixedRelease {
        package: PackageName,
        version: String,
        advisory: String,
    },
    #[error("{advisory} affects every version of {package}, so no constraint can avoid it")]
    VersionUnbounded {
        package: PackageName,
        advisory: String,
    },
    #[error("the fix data of {advisory} for {package} cannot be interpreted: {reason}")]
    UnsupportedFixData {
        package: PackageName,
        advisory: String,
        reason: String,
    },
    #[error(
        "every fixed release of {package} reported by {} is affected by another of them",
        .advisories.join(", ")
    )]
    NoSafeRelease {
        package: PackageName,
        advisories: Vec<String>,
    },
    #[error("no safe release of {package} satisfies the manifest requirement '{requirement}'")]
    IncompatibleRequirement {
        package: PackageName,
        requirement: String,
    },
}

/// The temporary constraints for every package with a policy-failing match.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemediationPlan {
    pub constraints: Vec<TemporaryConstraint>,
}

impl RemediationPlan {
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }
}

/// Derives temporary constraints for every match of `report` that fails
/// `policy`. Either every such match gets a constraint or the blockers are
/// returned: a partial plan cannot make the audit pass.
pub fn plan(
    report: &AuditReport,
    policy: &Policy,
    requirements: &[ManifestRequirement],
) -> Result<RemediationPlan, Vec<PlanBlocker>> {
    let mut by_package: BTreeMap<&PackageName, Vec<&Match>> = BTreeMap::new();
    for vuln_match in policy.failing(report) {
        by_package
            .entry(&vuln_match.package.name)
            .or_default()
            .push(vuln_match);
    }

    let mut constraints = Vec::new();
    let mut blockers = Vec::new();
    for (package, findings) in by_package {
        let requirement_specs: Vec<&str> = requirements
            .iter()
            .filter(|requirement| requirement.package == *package)
            .map(|requirement| requirement.spec.as_str())
            .collect();
        let advisories = advisory_ids(&findings);
        let result = match package {
            PackageName::Conda(name) => {
                plan_package::<CondaVersion>(package, &findings, &requirement_specs, Shape::Union)
                    .map(|components| TemporaryConstraint::Conda {
                        name: name.clone(),
                        version: conda_spec(&components),
                        advisories,
                    })
            }
            PackageName::Pypi(name) => plan_package::<PypiVersion>(
                package,
                &findings,
                &requirement_specs,
                Shape::SingleRange,
            )
            .map(|components| TemporaryConstraint::Pypi {
                name: name.clone(),
                specifiers: pypi_specifiers(&components),
                advisories,
            }),
        };
        match result {
            Ok(constraint) => constraints.push(constraint),
            Err(package_blockers) => blockers.extend(package_blockers),
        }
    }

    if blockers.is_empty() {
        Ok(RemediationPlan { constraints })
    } else {
        Err(blockers)
    }
}

fn advisory_ids(findings: &[&Match]) -> Vec<String> {
    let mut ids: Vec<String> = findings.iter().map(|m| m.id.clone()).collect();
    ids.sort();
    ids.dedup();
    ids
}

/// Which constraints an ecosystem can express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A union of ranges, like conda's `|`.
    Union,
    /// A single range with single-version exclusions, like PEP 440.
    SingleRange,
}

/// A version type that advisory and manifest versions parse into.
trait EcosystemVersion: Ord + Clone + FromStr {
    /// Parses a manifest requirement into the versions it allows, or `None`
    /// when its operators have no exact interval form here.
    fn requirement(spec: &str) -> Option<IntervalSet<Self>>;
}

impl EcosystemVersion for CondaVersion {
    fn requirement(spec: &str) -> Option<IntervalSet<Self>> {
        let spec = VersionSpec::from_str(spec, ParseStrictness::Lenient).ok()?;
        conda_requirement(&spec)
    }
}

impl EcosystemVersion for PypiVersion {
    fn requirement(spec: &str) -> Option<IntervalSet<Self>> {
        let spec = spec.trim();
        if spec.is_empty() || spec == "*" {
            return Some(IntervalSet::full());
        }
        let specifiers = VersionSpecifiers::from_str(spec).ok()?;
        specifiers
            .iter()
            .try_fold(IntervalSet::full(), |set, specifier| {
                Some(set.intersect(&pypi_requirement(specifier)?))
            })
    }
}

fn conda_requirement(spec: &VersionSpec) -> Option<IntervalSet<CondaVersion>> {
    let interval = |lo, hi| IntervalSet::from_intervals([Interval { lo, hi }]);
    Some(match spec {
        VersionSpec::Any => IntervalSet::full(),
        VersionSpec::None => IntervalSet::empty(),
        VersionSpec::Range(operator, version) => {
            let version = version.clone();
            match operator {
                RangeOperator::Greater => interval(Bound::Excluded(version), Bound::Unbounded),
                RangeOperator::GreaterEquals => {
                    interval(Bound::Included(version), Bound::Unbounded)
                }
                RangeOperator::Less => interval(Bound::Unbounded, Bound::Excluded(version)),
                RangeOperator::LessEquals => interval(Bound::Unbounded, Bound::Included(version)),
            }
        }
        VersionSpec::Exact(EqualityOperator::Equals, version) => {
            IntervalSet::point(version.clone())
        }
        VersionSpec::Exact(EqualityOperator::NotEquals, version) => {
            IntervalSet::point(version.clone()).complement()
        }
        VersionSpec::Group(LogicalOperator::And, specs) => {
            specs.iter().try_fold(IntervalSet::full(), |set, spec| {
                Some(set.intersect(&conda_requirement(spec)?))
            })?
        }
        VersionSpec::Group(LogicalOperator::Or, specs) => {
            specs.iter().try_fold(IntervalSet::empty(), |set, spec| {
                Some(set.union(&conda_requirement(spec)?))
            })?
        }
        // Prefix and compatible-release matches have no exact interval here.
        VersionSpec::StrictRange(..) => return None,
    })
}

fn pypi_requirement(specifier: &VersionSpecifier) -> Option<IntervalSet<PypiVersion>> {
    let version = specifier.version().clone();
    let interval = |lo, hi| IntervalSet::from_intervals([Interval { lo, hi }]);
    Some(match specifier.operator() {
        Operator::Equal | Operator::ExactEqual => IntervalSet::point(version),
        Operator::NotEqual => IntervalSet::point(version).complement(),
        Operator::LessThan => interval(Bound::Unbounded, Bound::Excluded(version)),
        Operator::LessThanEqual => interval(Bound::Unbounded, Bound::Included(version)),
        Operator::GreaterThan => interval(Bound::Excluded(version), Bound::Unbounded),
        Operator::GreaterThanEqual => interval(Bound::Included(version), Bound::Unbounded),
        // Wildcards and compatible releases have no exact interval here.
        _ => return None,
    })
}

/// What one advisory record says, in versions of one ecosystem.
struct Interpretation<V> {
    affected: IntervalSet<V>,
    /// Lower bounds of the versions after a documented boundary: `[fixed`,
    /// or `(last_affected`, after which the record reports no affected
    /// version.
    fix_boundaries: Vec<Bound<V>>,
}

fn parse<V: FromStr>(version: &str) -> Result<V, String> {
    V::from_str(version.trim()).map_err(|_| format!("'{version}' is not a valid version"))
}

/// Interprets one record. Each range is evaluated on its own: events sorted
/// by version, an `introduced` opens an affected interval, a `fixed` closes
/// it before the fix, a `last_affected` after the last affected version.
/// Listed versions are affected individually.
fn interpret<V: EcosystemVersion>(
    evidence: &AdvisoryEvidence,
) -> Result<Interpretation<V>, String> {
    let mut intervals = Vec::new();
    let mut fix_boundaries = Vec::new();
    for range in &evidence.ranges {
        let mut events = Vec::with_capacity(range.events.len());
        for event in &range.events {
            let (kind, version) = match event {
                RangeEvent::Introduced(version) if version.trim() == "0" => {
                    (EventKind::Introduced, None)
                }
                RangeEvent::Introduced(version) => (EventKind::Introduced, Some(version)),
                RangeEvent::Fixed(version) => (EventKind::Fixed, Some(version)),
                RangeEvent::LastAffected(version) => (EventKind::LastAffected, Some(version)),
            };
            let version = version.map(|version| parse::<V>(version)).transpose()?;
            events.push((version, kind));
        }
        // `None` (introduced "0") sorts first. The sort is stable, so events
        // at the same version keep their record order.
        events.sort_by(|a, b| a.0.cmp(&b.0));

        let mut open: Option<Bound<V>> = None;
        for (version, kind) in events {
            match (kind, version) {
                (EventKind::Introduced, version) => {
                    if open.is_none() {
                        open = Some(version.map_or(Bound::Unbounded, Bound::Included));
                    }
                }
                (EventKind::Fixed, Some(version)) => {
                    fix_boundaries.push(Bound::Included(version.clone()));
                    if let Some(lo) = open.take() {
                        intervals.push(Interval {
                            lo,
                            hi: Bound::Excluded(version),
                        });
                    }
                }
                (EventKind::LastAffected, Some(version)) => {
                    fix_boundaries.push(Bound::Excluded(version.clone()));
                    if let Some(lo) = open.take() {
                        intervals.push(Interval {
                            lo,
                            hi: Bound::Included(version),
                        });
                    }
                }
                (EventKind::Fixed | EventKind::LastAffected, None) => {
                    unreachable!("only introduced events can be unversioned")
                }
            }
        }
        if let Some(lo) = open {
            intervals.push(Interval {
                lo,
                hi: Bound::Unbounded,
            });
        }
    }
    let mut affected = IntervalSet::from_intervals(intervals);
    // A listed version that does not parse in the ecosystem cannot be a
    // release of it, so it cannot be selected either.
    for version in &evidence.versions {
        if let Ok(version) = parse::<V>(version) {
            affected = affected.union(&IntervalSet::point(version));
        }
    }
    Ok(Interpretation {
        affected,
        fix_boundaries,
    })
}

#[derive(Debug, Clone, Copy)]
enum EventKind {
    Introduced,
    Fixed,
    LastAffected,
}

/// The safe ranges for one package that begin at a documented fix and leave
/// room for the manifest requirements.
fn plan_package<V: EcosystemVersion>(
    package: &PackageName,
    findings: &[&Match],
    requirement_specs: &[&str],
    shape: Shape,
) -> Result<Vec<Component<V>>, Vec<PlanBlocker>> {
    let mut blockers = Vec::new();
    let mut affected = IntervalSet::<V>::empty();
    // The documented fixes of each finding, across its records.
    let mut fixes_per_finding: Vec<Vec<Bound<V>>> = Vec::with_capacity(findings.len());
    for finding in findings {
        if finding.match_confidence == MatchConfidence::VersionUnbounded {
            blockers.push(PlanBlocker::VersionUnbounded {
                package: package.clone(),
                advisory: finding.id.clone(),
            });
            continue;
        }
        // The database reported the locked version, whatever the records
        // spell out.
        let locked = match parse::<V>(&finding.package.version) {
            Ok(locked) => {
                affected = affected.union(&IntervalSet::point(locked.clone()));
                Some(locked)
            }
            Err(reason) => {
                blockers.push(PlanBlocker::UnsupportedFixData {
                    package: package.clone(),
                    advisory: finding.id.clone(),
                    reason,
                });
                None
            }
        };
        let mut fix_boundaries = Vec::new();
        let blocked_before = blockers.len();
        for evidence in &finding.advisories {
            match interpret::<V>(evidence) {
                Ok(interpretation) => {
                    affected = affected.union(&interpretation.affected);
                    // Only a fix after the locked version fixes it: the fix
                    // of an older release branch would be a downgrade.
                    fix_boundaries.extend(interpretation.fix_boundaries.into_iter().filter(
                        |fix| {
                            locked.as_ref().is_some_and(|locked| match fix {
                                Bound::Included(fixed) => fixed > locked,
                                Bound::Excluded(last_affected) => last_affected >= locked,
                                Bound::Unbounded => false,
                            })
                        },
                    ));
                }
                Err(reason) => blockers.push(PlanBlocker::UnsupportedFixData {
                    package: package.clone(),
                    advisory: evidence.id.clone(),
                    reason,
                }),
            }
        }
        if fix_boundaries.is_empty() && blockers.len() == blocked_before {
            blockers.push(PlanBlocker::NoFixedRelease {
                package: package.clone(),
                version: finding.package.version.clone(),
                advisory: finding.id.clone(),
            });
        }
        fixes_per_finding.push(fix_boundaries);
    }
    if !blockers.is_empty() {
        return Err(blockers);
    }

    // A safe range is used only when it starts at or after a documented fix
    // newer than the locked version of one of the findings: versions below
    // every such fix, or merely not mentioned, are not remediation. Ranges
    // stay separate per release branch, so findings locked on different
    // branches can each move to the fix of their own branch.
    let justified: Vec<Component<V>> = affected
        .complement()
        .components()
        .into_iter()
        .filter(|component| {
            fixes_per_finding.iter().any(|fixes| {
                fixes
                    .iter()
                    .any(|fix| ranges::cmp_lo(fix, &component.lo).is_le())
            })
        })
        .collect();
    if justified.is_empty() {
        return Err(vec![PlanBlocker::NoSafeRelease {
            package: package.clone(),
            advisories: advisory_ids(findings),
        }]);
    }

    // Requirements without an exact interval form are left to the solver.
    let requirements: Vec<(&str, IntervalSet<V>)> = requirement_specs
        .iter()
        .filter_map(|spec| Some((*spec, V::requirement(spec)?)))
        .collect();

    match shape {
        // Requirements belong to individual environments, and an
        // environment may depend on the package only transitively, so every
        // justified range is kept and the solver picks within them.
        Shape::Union => {
            let kept = justified;
            let unmet: Vec<PlanBlocker> = requirements
                .iter()
                .filter(|(_, allowed)| {
                    !kept
                        .iter()
                        .any(|component| allowed.overlaps(&component.as_set()))
                })
                .map(|(spec, _)| PlanBlocker::IncompatibleRequirement {
                    package: package.clone(),
                    requirement: spec.to_string(),
                })
                .collect();
            if unmet.is_empty() {
                Ok(kept)
            } else {
                Err(unmet)
            }
        }
        // The highest range only adds a lower bound, where a lower range
        // would also cap upgrades the solver would otherwise prefer.
        Shape::SingleRange => justified
            .into_iter()
            .rev()
            .find(|component| {
                requirements
                    .iter()
                    .all(|(_, allowed)| allowed.overlaps(&component.as_set()))
            })
            .map(|component| vec![component])
            .ok_or_else(|| {
                requirements
                    .iter()
                    .map(|(spec, _)| PlanBlocker::IncompatibleRequirement {
                        package: package.clone(),
                        requirement: spec.to_string(),
                    })
                    .collect()
            }),
    }
}

fn conda_spec(components: &[Component<CondaVersion>]) -> VersionSpec {
    let mut alternatives: Vec<VersionSpec> =
        components
            .iter()
            .map(|component| {
                let mut parts = Vec::new();
                match &component.lo {
                    Bound::Included(version) => {
                        parts.push(VersionSpec::Range(
                            RangeOperator::GreaterEquals,
                            version.clone(),
                        ));
                    }
                    Bound::Excluded(version) => {
                        parts.push(VersionSpec::Range(RangeOperator::Greater, version.clone()));
                    }
                    Bound::Unbounded => {}
                }
                match &component.hi {
                    Bound::Included(version) => {
                        parts.push(VersionSpec::Range(
                            RangeOperator::LessEquals,
                            version.clone(),
                        ));
                    }
                    Bound::Excluded(version) => {
                        parts.push(VersionSpec::Range(RangeOperator::Less, version.clone()));
                    }
                    Bound::Unbounded => {}
                }
                parts.extend(component.excluded.iter().map(|version| {
                    VersionSpec::Exact(EqualityOperator::NotEquals, version.clone())
                }));
                match parts.len() {
                    0 => VersionSpec::Any,
                    1 => parts.pop().expect("one part"),
                    _ => VersionSpec::Group(LogicalOperator::And, parts),
                }
            })
            .collect();
    match alternatives.len() {
        1 => alternatives.pop().expect("one alternative"),
        _ => VersionSpec::Group(LogicalOperator::Or, alternatives),
    }
}

/// PEP 440 specifiers for a range. PEP 440 cannot express a union, so
/// [`Shape::SingleRange`] planning hands over exactly one range.
fn pypi_specifiers(components: &[Component<PypiVersion>]) -> VersionSpecifiers {
    let [component] = components else {
        unreachable!("single-range planning yields one range")
    };
    let specifier = |operator, version: &PypiVersion| {
        VersionSpecifier::from_version(operator, version.clone())
            .expect("ordered comparisons accept every version")
    };
    let mut specifiers = Vec::new();
    match &component.lo {
        Bound::Included(version) => specifiers.push(specifier(Operator::GreaterThanEqual, version)),
        Bound::Excluded(version) => specifiers.push(specifier(Operator::GreaterThan, version)),
        Bound::Unbounded => {}
    }
    match &component.hi {
        Bound::Included(version) => specifiers.push(specifier(Operator::LessThanEqual, version)),
        Bound::Excluded(version) => specifiers.push(specifier(Operator::LessThan, version)),
        Bound::Unbounded => {}
    }
    specifiers.extend(
        component
            .excluded
            .iter()
            .map(|version| specifier(Operator::NotEqual, version)),
    );
    specifiers.into_iter().collect()
}

#[cfg(test)]
mod tests;
