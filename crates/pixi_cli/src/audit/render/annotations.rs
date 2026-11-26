//! GitHub Actions workflow commands. Written to stderr on a runner, they
//! surface every vulnerability match as an annotation on the job and on the pull
//! request's changed files, without touching the report on stdout.

use std::fmt::Write as _;

use pixi_audit::{AuditReport, Match};

use super::{confidence_note, fix_summary, risk_label};

/// The annotation level, chosen by whether the audit fails the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::audit) enum Level {
    Error,
    Warning,
}

impl Level {
    fn command(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

/// Whether this process runs inside a GitHub Actions job.
pub(in crate::audit) fn on_github_actions() -> bool {
    std::env::var_os("GITHUB_ACTIONS").is_some_and(|value| value == "true")
}

/// One workflow command per vulnerability, each on its own line.
pub(in crate::audit) fn github_annotations(
    report: &AuditReport,
    level_for: impl Fn(&Match) -> Level,
) -> String {
    let mut out = String::new();
    for vuln_match in &report.matches {
        writeln!(
            out,
            "::{} title={}::{}",
            level_for(vuln_match).command(),
            escape_property(&format!(
                "{} in {} {}",
                vuln_match.id, vuln_match.package.name, vuln_match.package.version
            )),
            escape_data(&message(vuln_match))
        )
        .expect("writing to a string");
    }
    out
}

fn message(vuln_match: &Match) -> String {
    let fix = fix_summary(vuln_match);
    let confidence_note = confidence_note(vuln_match)
        .map(|note| format!(", {note}"))
        .unwrap_or_default();
    let mut text = format!("{} severity", vuln_match.severity);
    if let Some(risk) = risk_label(vuln_match.risk) {
        text.push_str(&format!(", risk {risk}"));
    }
    if !vuln_match.kev.is_empty() {
        text.push_str(", known exploited");
    }
    text.push_str(&format!(
        ", {fix}{confidence_note} ({}; {})",
        vuln_match.package.ecosystem,
        vuln_match.environments.join(", ")
    ));
    if let Some(summary) = &vuln_match.summary {
        text.push_str(": ");
        text.push_str(summary);
    }
    if let Some(url) = &vuln_match.url {
        text.push(' ');
        text.push_str(url);
    }
    text
}

/// Escapes a command's message per the runner's rules.
fn escape_data(text: &str) -> String {
    text.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a command's property value, which additionally cannot contain
/// the property delimiters.
fn escape_property(text: &str) -> String {
    escape_data(text).replace(':', "%3A").replace(',', "%2C")
}

#[cfg(test)]
mod tests {
    use pixi_audit::{
        MatchConfidence, PackageEcosystem, PackageId, PackageName, RiskScore, SeverityRating,
    };

    use super::*;

    fn finding() -> Match {
        Match {
            package: PackageId {
                name: PackageName::conda("openssl").expect("valid name"),
                version: "3.1.0".to_string(),
                ecosystem: PackageEcosystem::CondaForge,
            },
            environments: vec!["default".to_string()],
            id: "CVE-2026-1".to_string(),
            aliases: vec![],
            severity: SeverityRating::High,
            cvss_base_score: None,
            fixed_versions: vec!["3.1.1".to_string()],
            advisories: vec![],
            other_ecosystem_fixes: vec![],
            upstream_releases: vec![],
            match_confidence: MatchConfidence::Unspecified,
            matched_identity: None,
            summary: Some("100% broken\nreally".to_string()),
            url: Some("https://osv.dev/CVE-2026-1".to_string()),
            epss: vec![],
            kev: vec![],
            risk: RiskScore::NONE,
            ignore_mode: None,
        }
    }

    #[test]
    fn one_command_per_match_with_escaped_title() {
        let report = AuditReport {
            matches: vec![finding()],
            ..AuditReport::default()
        };
        insta::assert_snapshot!(
            github_annotations(&report, |_| Level::Error),
            @"::error title=CVE-2026-1 in openssl 3.1.0::high severity, fixed in 3.1.1, match details unavailable (conda-forge; default): 100%25 broken%0Areally https://osv.dev/CVE-2026-1"
        );
        assert!(github_annotations(&report, |_| Level::Warning).starts_with("::warning "));
    }
}
