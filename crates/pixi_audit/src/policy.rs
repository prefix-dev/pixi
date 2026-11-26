//! Whether an audit report passes the selected policy. Shared by the normal
//! audit, remediation target selection, and remediation candidate acceptance,
//! so all three agree on which vulnerability matches fail.

use crate::report::{AuditReport, Match, PackageEcosystem, SeverityRating};

/// The parts of the policy applied after ignore rules: which remaining
/// vulnerability matches fail the audit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    /// Fail only for matches at or above this rating. Unknown severity always
    /// fails, so unscored advisories never slip under a threshold.
    pub fail_on: Option<SeverityRating>,
    /// Ecosystems whose matches are reported without failing the audit.
    pub warn_only: Vec<PackageEcosystem>,
}

/// What a report means for the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// At least one match fails the policy.
    Fail { failing: usize },
    /// Matches remain, but every eligible one sits below `rating`.
    BelowThreshold {
        rating: SeverityRating,
        count: usize,
    },
    /// No match can fail the audit; warning-only matches may exist.
    Pass,
}

impl Verdict {
    pub fn passes(self) -> bool {
        !matches!(self, Verdict::Fail { .. })
    }
}

impl Policy {
    /// Whether the policy reports matches in this ecosystem without failing.
    fn warns_only(&self, ecosystem: &PackageEcosystem) -> bool {
        self.warn_only.contains(ecosystem)
    }

    fn meets_threshold(&self, vuln_match: &Match) -> bool {
        self.fail_on.is_none_or(|rating| {
            vuln_match.severity == SeverityRating::Unknown || vuln_match.severity >= rating
        })
    }

    /// Whether `vuln_match`, which no ignore rule covers, fails the audit.
    pub fn fails(&self, vuln_match: &Match) -> bool {
        !self.warns_only(&vuln_match.package.ecosystem) && self.meets_threshold(vuln_match)
    }

    /// The matches of `report` that fail the audit.
    pub fn failing<'a>(&self, report: &'a AuditReport) -> impl Iterator<Item = &'a Match> {
        report
            .matches
            .iter()
            .filter(move |vuln_match| self.fails(vuln_match))
    }

    pub fn evaluate(&self, report: &AuditReport) -> Verdict {
        let failing = self.failing(report).count();
        if failing > 0 {
            return Verdict::Fail { failing };
        }
        let eligible = report
            .matches
            .iter()
            .filter(|vuln_match| !self.warns_only(&vuln_match.package.ecosystem))
            .count();
        match self.fail_on {
            Some(rating) if eligible > 0 => Verdict::BelowThreshold {
                rating,
                count: eligible,
            },
            _ => Verdict::Pass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PackageId, PackageName};

    fn finding(ecosystem: PackageEcosystem, severity: SeverityRating) -> Match {
        let name = match ecosystem {
            PackageEcosystem::Pypi => PackageName::pypi("requests").unwrap(),
            _ => PackageName::conda("openssl").unwrap(),
        };
        Match {
            package: PackageId {
                name,
                version: "1.0".to_string(),
                ecosystem,
            },
            environments: vec!["default".to_string()],
            id: "CVE-1".to_string(),
            aliases: vec![],
            severity,
            cvss_base_score: None,
            fixed_versions: vec![],
            advisories: vec![],
            other_ecosystem_fixes: vec![],
            upstream_releases: vec![],
            match_confidence: crate::MatchConfidence::Unspecified,
            matched_identity: None,
            summary: None,
            url: None,
            epss: vec![],
            kev: vec![],
            risk: crate::RiskScore::NONE,
            ignore_mode: None,
        }
    }

    fn report(matches: Vec<Match>) -> AuditReport {
        AuditReport {
            matches,
            ..AuditReport::default()
        }
    }

    #[test]
    fn unknown_severity_fails_any_threshold() {
        let policy = Policy {
            fail_on: Some(SeverityRating::Critical),
            ..Policy::default()
        };
        let verdict = policy.evaluate(&report(vec![finding(
            PackageEcosystem::CondaForge,
            SeverityRating::Unknown,
        )]));
        assert_eq!(verdict, Verdict::Fail { failing: 1 });
    }

    #[test]
    fn matches_below_the_threshold_pass() {
        let policy = Policy {
            fail_on: Some(SeverityRating::High),
            ..Policy::default()
        };
        let verdict = policy.evaluate(&report(vec![finding(
            PackageEcosystem::CondaForge,
            SeverityRating::Medium,
        )]));
        assert_eq!(
            verdict,
            Verdict::BelowThreshold {
                rating: SeverityRating::High,
                count: 1
            }
        );
        assert!(verdict.passes());
    }

    #[test]
    fn warning_only_ecosystems_never_fail() {
        let policy = Policy {
            warn_only: vec![PackageEcosystem::Pypi],
            ..Policy::default()
        };
        let pypi = finding(PackageEcosystem::Pypi, SeverityRating::Critical);
        assert_eq!(policy.evaluate(&report(vec![pypi.clone()])), Verdict::Pass);
        let conda = finding(PackageEcosystem::CondaForge, SeverityRating::Low);
        assert_eq!(
            policy.evaluate(&report(vec![pypi, conda])),
            Verdict::Fail { failing: 1 }
        );
    }
}
