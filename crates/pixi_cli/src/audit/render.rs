//! Human-readable reports and the labels shared by every report format.

use std::{collections::BTreeSet, fmt::Write as _};

use console::style;
use pixi_audit::{
    AuditReport, CondaForgeBuild, EpssScore, FixAvailability, IgnoreMode, KevEntry, Match,
    MatchConfidence, OtherEcosystemFix, PackageEcosystem, PackageId, RansomwareCampaignUse,
    RiskScore, SeverityRating, ThreatSourceStatus, UpstreamRelease,
};

pub(super) mod annotations;
pub(super) mod sarif;
pub(super) mod table;

/// Whether the report describes the result of `--fix`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum FixState {
    /// `--fix` was not requested.
    #[default]
    NotRequested,
    /// `--fix` was requested and the report describes the unchanged
    /// workspace, because remediation was not possible or not accepted.
    NotApplied,
    /// `--fix` replaced the lock file and the report describes the result.
    Applied,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct GuidanceOptions {
    pub fix: FixState,
    pub explain: bool,
}

/// Grouped by package, one line per vulnerability match.
pub(super) fn render_by_package(report: &AuditReport, options: GuidanceOptions) -> String {
    let mut out = String::new();
    if !report.not_audited.is_empty() {
        writeln!(
            out,
            "{} package(s) could not be audited:",
            report.not_audited.len()
        )
        .expect("writing to a string");
        for package in &report.not_audited {
            writeln!(
                out,
                "  {} {} ({}) — {}",
                package.name,
                package.version.as_deref().unwrap_or("?"),
                package.ecosystem,
                style(package.environments.join(", ")).magenta()
            )
            .expect("writing to a string");
        }
        out.push('\n');
    }

    let columns = ThreatColumns::for_report(report);
    for group in grouped(&report.matches) {
        writeln!(out, "{}", group.header).expect("writing to a string");
        for vuln_match in group.members {
            let severity = severity_label(vuln_match.severity)
                .map(|cell| format!("  {cell}"))
                .unwrap_or_default();
            writeln!(
                out,
                "  {}{severity}{}  {}{}",
                vuln_match.id,
                columns.cells(vuln_match),
                fix_summary(vuln_match),
                confidence_note(vuln_match)
                    .map(|note| format!("  {}", style(note).dim()))
                    .unwrap_or_default()
            )
            .expect("writing to a string");
            if let Some(url) = &vuln_match.url {
                writeln!(out, "  {}", style(url).dim()).expect("writing to a string");
            }
        }
        out.push('\n');
    }

    if !report.ignored.is_empty() {
        writeln!(
            out,
            "Ignored {} vulnerability match(es):",
            report.ignored.len()
        )
        .expect("writing to a string");
        for vuln_match in &report.ignored {
            let marker = match vuln_match.ignore_mode {
                Some(IgnoreMode::UntilFixed) => " — until a fix is released",
                Some(IgnoreMode::Permanent) | None => "",
            };
            writeln!(
                out,
                "  {} ({} {}){marker}",
                vuln_match.id, vuln_match.package.name, vuln_match.package.version
            )
            .expect("writing to a string");
        }
        out.push('\n');
    }

    writeln!(out, "{}", summary_line(report)).expect("writing to a string");
    render_guidance(&mut out, report, options);
    out
}

/// `✔ No vulnerabilities found (…)` or `✘ N vulnerabilities in M package(s) (…)`.
pub(super) fn summary_line(report: &AuditReport) -> String {
    let summary = &report.summary;
    let details = style(format!(
        "({} audited, {} ignored, {} not audited)",
        summary.audited, summary.ignored, summary.not_audited
    ))
    .dim();
    if summary.vulnerable == 0 {
        return format!(
            "{}No vulnerabilities found {details}",
            style(console::Emoji("✔ ", "")).green().bold()
        );
    }
    let packages: BTreeSet<&PackageId> = report.matches.iter().map(|m| &m.package).collect();
    format!(
        "{} {details}",
        style(format!(
            "{}{} in {}",
            console::Emoji("✘ ", ""),
            count(summary.vulnerable, "vulnerability", "vulnerabilities"),
            count(packages.len(), "package", "packages")
        ))
        .red()
        .bold()
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KevSummary {
    Loaded(usize),
    Unavailable,
    NotChecked,
}

pub(super) const EXPLAIN_HINT: &str = "Add `--explain` for scoring and report definitions.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExplanationEntry {
    pub label: &'static str,
    pub text: &'static str,
    pub indent: usize,
}

impl ExplanationEntry {
    const fn new(label: &'static str, text: &'static str) -> Self {
        Self {
            label,
            text,
            indent: 0,
        }
    }

    const fn child(label: &'static str, text: &'static str) -> Self {
        Self {
            label,
            text,
            indent: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExplanationSection {
    pub title: &'static str,
    pub entries: Vec<ExplanationEntry>,
}

impl ExplanationSection {
    fn label_width(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.label.len() + entry.indent)
            .max()
            .unwrap_or(0)
            + 2
    }
}

/// The overview and recommendations below a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Guidance {
    highest_risk: Option<String>,
    kev: KevSummary,
    severe: usize,
    fixable_matches: usize,
    audited_packages_with_fixes: usize,
    elsewhere_only: usize,
    no_fix: usize,
    ignored: usize,
    not_audited: usize,
    total: usize,
}

pub(super) fn count(number: usize, singular: &str, plural: &str) -> String {
    format!("{number} {}", if number == 1 { singular } else { plural })
}

impl Guidance {
    pub fn for_report(report: &AuditReport) -> Self {
        let matches = &report.matches;
        let kev_cves = matches
            .iter()
            .flat_map(|vuln_match| &vuln_match.kev)
            .map(|entry| entry.cve.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let fix_count = |status: FixAvailability| {
            matches
                .iter()
                .filter(|m| m.fix_availability() == status)
                .count()
        };
        Self {
            highest_risk: matches.iter().map(|m| m.risk).max().and_then(risk_label),
            kev: match &report.threat_data.kev {
                ThreatSourceStatus::Loaded { .. } => KevSummary::Loaded(kev_cves),
                ThreatSourceStatus::Failed { .. } => KevSummary::Unavailable,
                ThreatSourceStatus::Disabled => KevSummary::NotChecked,
            },
            severe: matches
                .iter()
                .filter(|m| matches!(m.severity, SeverityRating::Critical | SeverityRating::High))
                .count(),
            fixable_matches: fix_count(FixAvailability::Available),
            audited_packages_with_fixes: matches
                .iter()
                .filter(|m| m.fix_availability() == FixAvailability::Available)
                .map(|m| &m.package)
                .collect::<BTreeSet<_>>()
                .len(),
            elsewhere_only: fix_count(FixAvailability::ElsewhereOnly),
            no_fix: fix_count(FixAvailability::None),
            ignored: report.ignored.len(),
            not_audited: report.not_audited.len(),
            total: matches.len(),
        }
    }

    pub fn assessment_lines(&self) -> Vec<String> {
        if self.total == 0 {
            return Vec::new();
        }
        let line = |label: &str, value: String| format!("{label:<39}{value}");
        let kev = match self.kev {
            KevSummary::Loaded(0) => "0 CVEs".to_string(),
            KevSummary::Loaded(number) => format!(
                "{} — exploitation confirmed in the wild",
                count(number, "CVE", "CVEs")
            ),
            KevSummary::Unavailable => "n/a — CISA KEV catalog unavailable".to_string(),
            KevSummary::NotChecked => "n/a — CISA KEV catalog not checked".to_string(),
        };
        vec![
            line(
                "Highest risk score:",
                self.highest_risk
                    .as_deref()
                    .map(|score| format!("{score} / 100"))
                    .unwrap_or_else(|| "n/a".to_string()),
            ),
            line("CISA KEV matches:", kev),
            line(
                "Critical/high vulnerability matches:",
                self.severe.to_string(),
            ),
            line(
                "Fix available in audited ecosystem:",
                format!(
                    "{} of {} across {}",
                    self.fixable_matches,
                    count(self.total, "vulnerability", "vulnerabilities"),
                    count(
                        self.audited_packages_with_fixes,
                        "audited package",
                        "audited packages"
                    )
                ),
            ),
            line(
                "Fix only elsewhere:",
                count(self.elsewhere_only, "vulnerability", "vulnerabilities"),
            ),
            line(
                "No known fix:",
                count(self.no_fix, "vulnerability", "vulnerabilities"),
            ),
        ]
    }

    pub fn next_step_lines(&self, fix: FixState) -> Vec<String> {
        let mut lines = Vec::new();
        if self.total == 0 {
            if fix == FixState::Applied {
                lines.push(
                    "The lock file was updated. The temporary constraints are not saved, so a later unconstrained solve can select a vulnerable release again. Add lasting constraints to the manifest where needed.".to_string(),
                );
            }
            if self.ignored == 0 && self.not_audited == 0 {
                if fix != FixState::Applied {
                    lines.push(
                        "None. No vulnerabilities were found in audited packages.".to_string(),
                    );
                }
                return lines;
            }
            lines.push("No active vulnerabilities were found in audited packages.".to_string());
            if self.ignored > 0 {
                lines.push(format!(
                    "Review {} periodically.",
                    count(
                        self.ignored,
                        "ignored vulnerability match",
                        "ignored vulnerability matches"
                    )
                ));
            }
        } else {
            if let KevSummary::Loaded(number) = self.kev
                && number > 0
            {
                lines.push(format!(
                    "Prioritize {} because exploitation has been confirmed in the wild.",
                    count(number, "CISA KEV match", "CISA KEV matches")
                ));
            }
            if self.fixable_matches > 0 && fix == FixState::NotRequested {
                lines.push(format!(
                    "Re-run this audit with `--fix` to update the lock file to fixed releases. Fixed releases in the audited ecosystem are reported for {}.",
                    count(self.fixable_matches, "vulnerability", "vulnerabilities")
                ));
            }
            if self.elsewhere_only > 0 {
                let (fix, pronoun) = if self.elsewhere_only == 1 {
                    ("a fix", "it")
                } else {
                    ("fixes", "them")
                };
                lines.push(format!(
                    "{} {} {fix} only elsewhere. `--fix` cannot install {pronoun}. Wait for an ecosystem build or change the dependency source.",
                    count(self.elsewhere_only, "vulnerability", "vulnerabilities"),
                    if self.elsewhere_only == 1 { "has" } else { "have" }
                ));
            }
            if self.no_fix > 0 {
                lines.push(format!(
                    "{} {} no known fix. Remove, replace, or mitigate the affected dependencies. Use `pixi audit ignore <ID> --until-fixed` only after accepting the risk.",
                    count(self.no_fix, "vulnerability", "vulnerabilities"),
                    if self.no_fix == 1 { "has" } else { "have" }
                ));
            }
        }
        if self.not_audited > 0 {
            lines.push(if self.not_audited == 1 {
                "Review 1 package that was not audited. Its source is not covered by the configured vulnerability databases.".to_string()
            } else {
                format!(
                    "Review {} packages that were not audited. Their sources are not covered by the configured vulnerability databases.",
                    self.not_audited
                )
            });
        }
        lines
    }

    pub fn explanation_sections(&self, report: &AuditReport) -> Vec<ExplanationSection> {
        let mut sections = vec![ExplanationSection {
            title: "Counts",
            entries: vec![
                ExplanationEntry::new("Package", "One locked name, version, and source."),
                ExplanationEntry::new("Vulnerability match", "One advisory affecting a package."),
                ExplanationEntry::new(
                    "CVE IDs",
                    "Standard vulnerability IDs. One vulnerability match can reference several.",
                ),
            ],
        }];

        if self.total > 0 {
            let mut threat = vec![
                ExplanationEntry::new("EPSS", "Estimated chance of exploitation within 30 days."),
                ExplanationEntry::new(
                    "Percentile",
                    "Rank among scored CVEs. 90th is higher than 90% of them.",
                ),
                ExplanationEntry::new("KEV", "CISA confirms exploitation in the wild."),
                ExplanationEntry::new(
                    "Boost",
                    "KEV adds 5%. Known ransomware use raises this to 10%.",
                ),
            ];
            if report.matches.iter().any(|m| !m.kev.is_empty()) {
                threat.push(ExplanationEntry::new(
                    "Markers",
                    "`kev, ransomware` means CISA records known ransomware use. `kev` alone means CISA does not mark that use as Known.",
                ));
            }
            sections.push(ExplanationSection {
                title: "Threat data",
                entries: threat,
            });
            sections.push(ExplanationSection {
                title: "Risk",
                entries: vec![
                    ExplanationEntry::new(
                        "Risk",
                        "A 0–100 score for ordering individual vulnerability matches.",
                    ),
                    ExplanationEntry::child(
                        "Formula",
                        "min(threat × severity × boost, 1) × 100.",
                    ),
                    ExplanationEntry::new(
                        "Threat",
                        "1.0 for any KEV match. Otherwise the highest EPSS probability, or 0 without data.",
                    ),
                    ExplanationEntry::new(
                        "Severity",
                        "Critical .90, high .75, medium/unknown .50, low .30. Averaged equally with CVSS when available.",
                    ),
                ],
            });
            sections.push(ExplanationSection {
                title: "Fixes",
                entries: vec![
                    ExplanationEntry::new(
                        "Fix available",
                        "A fixed release exists for the audited package source. The `Fixed in` column lists it.",
                    ),
                    ExplanationEntry::new(
                        "Fix only elsewhere",
                        "A fix exists upstream or in another ecosystem. `--fix` cannot install it.",
                    ),
                    ExplanationEntry::new(
                        "No known fix",
                        "Audit sources list no fixed release. Mitigation may still be possible.",
                    ),
                    ExplanationEntry::new(
                        "`--fix`",
                        "Solves once with temporary constraints, audits the result, and replaces the lock file only when every finding then passes the policy. The manifest is never edited.",
                    ),
                ],
            });
        }

        let mut coverage = Vec::new();
        if self.ignored > 0 {
            coverage.push(ExplanationEntry::new(
                "Ignored",
                "Excluded from active totals and the failure decision. The package is not patched.",
            ));
        }
        if self.not_audited > 0 {
            coverage.push(ExplanationEntry::new(
                "Not audited",
                "The package source lacks vulnerability database coverage. Safety is unknown.",
            ));
        }
        let all = || report.matches.iter().chain(&report.ignored);
        if all().any(|m| m.match_confidence == MatchConfidence::VersionUnbounded) {
            coverage.push(ExplanationEntry::new(
                "No version limits",
                "The advisory has no usable version bounds, so every version matches.",
            ));
        }
        if all().any(|m| m.match_confidence == MatchConfidence::Unevaluated) {
            coverage.push(ExplanationEntry::new(
                "Version not checked",
                "The source lacks enough version data to decide whether this version is affected.",
            ));
        }
        if !coverage.is_empty() {
            sections.push(ExplanationSection {
                title: "Ignored and not audited",
                entries: coverage,
            });
        }
        sections
    }
}

/// Appends the overview, recommendations, and definitions or the hint to
/// request them.
pub(super) fn render_guidance(out: &mut String, report: &AuditReport, options: GuidanceOptions) {
    let guidance = Guidance::for_report(report);
    let assessment = guidance.assessment_lines();
    if !assessment.is_empty() {
        writeln!(out, "\n{}", style("Overview").bold()).expect("writing to a string");
        for line in assessment {
            writeln!(out, "  • {line}").expect("writing to a string");
        }
    }
    let next_steps = guidance.next_step_lines(options.fix);
    if !next_steps.is_empty() {
        writeln!(out, "\n{}", style("Recommendations").bold()).expect("writing to a string");
        for line in next_steps {
            writeln!(out, "  • {line}").expect("writing to a string");
        }
    }
    if options.explain {
        writeln!(out, "\n{}", style("Definitions").bold()).expect("writing to a string");
        for section in guidance.explanation_sections(report) {
            let width = section.label_width();
            writeln!(out, "\n  {}", style(section.title).bold()).expect("writing to a string");
            for entry in section.entries {
                let label = format!(
                    "{:indent$}{:<width$}",
                    "",
                    entry.label,
                    indent = entry.indent,
                    width = width - entry.indent
                );
                writeln!(out, "    {}{}", style(label).cyan().bold(), entry.text)
                    .expect("writing to a string");
            }
        }
    } else {
        writeln!(out, "\n{}", style(EXPLAIN_HINT).dim()).expect("writing to a string");
    }
}

/// The vulnerability matches for one locked package, under a shared header.
struct Group<'a> {
    key: &'a PackageId,
    header: String,
    members: Vec<&'a Match>,
}

/// Groups vulnerability matches by package in first-appearance order, which
/// keeps the riskiest packages first since the report is sorted by risk.
fn grouped(matches: &[Match]) -> Vec<Group<'_>> {
    let mut groups: Vec<Group<'_>> = Vec::new();
    for vuln_match in matches {
        let key = &vuln_match.package;
        match groups.iter_mut().find(|group| group.key == key) {
            Some(group) => group.members.push(vuln_match),
            None => groups.push(Group {
                key,
                header: format!(
                    "{} {} ({}) — {}",
                    style(&vuln_match.package.name).bold(),
                    vuln_match.package.version,
                    vuln_match.package.ecosystem,
                    style(vuln_match.environments.join(", ")).magenta()
                ),
                members: vec![vuln_match],
            }),
        }
    }
    groups
}

/// The style a severity rating carries wherever it is shown.
pub(super) fn severity_style(rating: SeverityRating) -> console::Style {
    match rating {
        SeverityRating::Critical => console::Style::new().magenta().bold(),
        SeverityRating::High => console::Style::new().red(),
        SeverityRating::Medium => console::Style::new().yellow(),
        SeverityRating::Low => console::Style::new().cyan(),
        SeverityRating::Unknown => console::Style::new().blue(),
    }
}

/// A fixed-width severity label, or `None` for unknown severity, which the
/// grouped report leaves blank rather than shouting "UNKNOWN".
fn severity_label(rating: SeverityRating) -> Option<String> {
    (rating != SeverityRating::Unknown).then(|| {
        severity_style(rating)
            .apply_to(format!("{:<8}", rating.name().to_uppercase()))
            .to_string()
    })
}

/// Which threat-data cells a report's sources justify. Risk is shown when
/// any source is configured, EPSS only when the EPSS source is.
#[derive(Clone, Copy)]
struct ThreatColumns {
    risk: bool,
    epss: bool,
}

impl ThreatColumns {
    fn for_report(report: &AuditReport) -> Self {
        let configured =
            |status: &ThreatSourceStatus| !matches!(status, ThreatSourceStatus::Disabled);
        let epss = configured(&report.threat_data.epss);
        Self {
            risk: epss || configured(&report.threat_data.kev),
            epss,
        }
    }

    fn cells(self, vuln_match: &Match) -> String {
        let mut cells = String::new();
        if self.risk {
            let label = risk_label(vuln_match.risk).unwrap_or_else(|| "n/a".to_string());
            cells.push_str(&format!("  risk {label:>5}"));
            if let Some(kev) = kev_label(&vuln_match.kev) {
                cells.push_str(&format!(" {}", kev_style().apply_to(format!("({kev})"))));
            }
        }
        if self.epss {
            let label = epss_label(&vuln_match.epss).unwrap_or_else(|| "n/a".to_string());
            cells.push_str(&format!("  epss {label}"));
        }
        cells
    }
}

pub(super) fn kev_style() -> console::Style {
    console::Style::new().magenta().bold()
}

/// The risk score with one decimal, `< 0.1` below that, or `None` when
/// there is no threat data behind it.
pub(super) fn risk_label(risk: RiskScore) -> Option<String> {
    let value = risk.value();
    if value == 0.0 {
        None
    } else if value < 0.1 {
        Some("< 0.1".to_string())
    } else {
        Some(format!("{value:.1}"))
    }
}

/// The percentile rank followed by exploit probability, such as
/// `99th · 90.8%`, or `None` without an EPSS record.
pub(super) fn epss_label(epss: &[EpssScore]) -> Option<String> {
    let score = highest_epss(epss)?;
    let percentile = ordinal((score.percentile * 100.0).floor() as u32);
    Some(format!("{percentile} · {}", probability(score)))
}

/// EPSS probability followed by percentile, for SARIF.
pub(super) fn epss_probability_first_label(epss: &[EpssScore]) -> Option<String> {
    let score = highest_epss(epss)?;
    let percentile = ordinal((score.percentile * 100.0).floor() as u32);
    Some(format!("{} ({percentile})", probability(score)))
}

fn probability(score: &EpssScore) -> String {
    let probability = score.epss * 100.0;
    if probability < 0.1 {
        "< 0.1%".to_string()
    } else {
        format!("{probability:.1}%")
    }
}

fn highest_epss(epss: &[EpssScore]) -> Option<&EpssScore> {
    epss.iter()
        .max_by(|left, right| left.epss.total_cmp(&right.epss))
}

fn ordinal(number: u32) -> String {
    // spellchecker:off
    let suffix = match (number % 100, number % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    // spellchecker:on
    format!("{number}{suffix}")
}

/// `kev`, or `kev, ransomware` when any entry reports ransomware use.
pub(super) fn kev_label(entries: &[KevEntry]) -> Option<&'static str> {
    if entries.is_empty() {
        return None;
    }
    let ransomware = entries
        .iter()
        .any(|entry| entry.known_ransomware_campaign_use == RansomwareCampaignUse::Known);
    Some(if ransomware { "kev, ransomware" } else { "kev" })
}

/// The match confidence as the Confidence column shows it.
pub(super) fn confidence_label(confidence: MatchConfidence) -> Option<&'static str> {
    match confidence {
        MatchConfidence::Evaluated => Some("affects version"),
        MatchConfidence::Unevaluated => Some("version not checked"),
        MatchConfidence::VersionUnbounded => Some("no version limits"),
        MatchConfidence::Unspecified => None,
    }
}

/// The label of a match worth flagging next to the advisory.
pub(super) fn weak_confidence_label(confidence: MatchConfidence) -> Option<&'static str> {
    match confidence {
        MatchConfidence::Unevaluated | MatchConfidence::VersionUnbounded => {
            confidence_label(confidence)
        }
        MatchConfidence::Evaluated | MatchConfidence::Unspecified => None,
    }
}

/// Describes the advisory's version match in human-readable output.
pub(super) fn confidence_note(vuln_match: &Match) -> Option<&'static str> {
    match vuln_match.match_confidence {
        MatchConfidence::VersionUnbounded => Some("confidence: no version limits"),
        MatchConfidence::Unevaluated => Some("confidence: version not checked"),
        MatchConfidence::Evaluated => Some("confidence: affects version"),
        MatchConfidence::Unspecified
            if vuln_match.package.ecosystem == PackageEcosystem::CondaForge =>
        {
            Some("match details unavailable")
        }
        MatchConfidence::Unspecified => None,
    }
}

/// Every fix outside the audited ecosystem spelled out, `; `-separated.
fn describe_fixes_elsewhere(vuln_match: &Match) -> String {
    vuln_match
        .other_ecosystem_fixes
        .iter()
        .map(|fix: &OtherEcosystemFix| format!("{} on {}", fix.version, fix.ecosystem))
        .chain(
            vuln_match
                .upstream_releases
                .iter()
                .map(describe_upstream_release),
        )
        .collect::<Vec<_>>()
        .join("; ")
}

fn describe_upstream_release(release: &UpstreamRelease) -> String {
    match release.conda_forge_build {
        CondaForgeBuild::Missing => {
            format!(
                "{} released upstream, no conda-forge build",
                release.version
            )
        }
        CondaForgeBuild::Unknown => format!(
            "{} released upstream, conda-forge build unconfirmed",
            release.version
        ),
        CondaForgeBuild::Available => format!("{} released upstream", release.version),
    }
}

/// Every fix outside the audited ecosystem as a short table label.
pub(super) fn fix_elsewhere_labels(vuln_match: &Match) -> Vec<String> {
    vuln_match
        .other_ecosystem_fixes
        .iter()
        .map(|fix| format!("{}: {}", fix.ecosystem, fix.version))
        .chain(
            vuln_match
                .upstream_releases
                .iter()
                .map(|release| match release.conda_forge_build {
                    CondaForgeBuild::Unknown => {
                        format!("upstream: {} (unconfirmed)", release.version)
                    }
                    CondaForgeBuild::Missing | CondaForgeBuild::Available => {
                        format!("upstream: {}", release.version)
                    }
                }),
        )
        .collect()
}

/// `fixed in 3.1.1`, `fix released elsewhere: ...`, or
/// `no known fixed version`.
pub(super) fn fix_summary(vuln_match: &Match) -> String {
    match vuln_match.fix_availability() {
        FixAvailability::Available => {
            format!("fixed in {}", vuln_match.fixed_versions.join(", "))
        }
        FixAvailability::ElsewhereOnly => format!(
            "fix released elsewhere: {}",
            describe_fixes_elsewhere(vuln_match)
        ),
        FixAvailability::None => "no known fixed version".to_string(),
    }
}
