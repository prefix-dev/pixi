//! The default report: a status block, then a `pixi list`-style table with
//! one row per vulnerability match.

use std::{cmp::Ordering, fmt::Write as _};

use clap::ValueEnum;
use comfy_table::{Cell, ContentArrangement, Table, presets::NOTHING};
use console::style;
use itertools::Itertools;
use pixi_audit::{
    AuditPackage, AuditReport, FixAvailability, IgnoreMode, Match, MatchConfidence,
    PackageEcosystem, SeverityRating, SortKey,
};
use pixi_consts::consts;

use super::{
    GuidanceOptions, confidence_label, epss_label, fix_elsewhere_labels, kev_label, kev_style,
    render_guidance, risk_label, severity_style, summary_line, weak_confidence_label,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Column {
    Name,
    Version,
    #[value(name = "fixed-in")]
    FixedIn,
    Kind,
    Source,
    Vulnerability,
    Severity,
    /// EPSS exploit probability and percentile.
    Epss,
    /// The 0-100 risk score.
    Risk,
    /// `kev` for matches in CISA's KEV catalog, `kev, ransomware` when used
    /// in ransomware campaigns.
    Kev,
    Environments,
    Aliases,
    Summary,
    Url,
    /// Why a match sits in the ignored section.
    Ignored,
    /// How the advisory's version data matched the package.
    Confidence,
}

/// Columns shown when `--columns` is not given, in this order.
pub const DEFAULT_COLUMNS: [Column; 8] = [
    Column::Name,
    Column::Version,
    Column::FixedIn,
    Column::Vulnerability,
    Column::Severity,
    Column::Epss,
    Column::Risk,
    Column::Environments,
];

/// Which columns the tables show, and in what order the rows come.
#[derive(Debug, Clone)]
pub struct TableLayout {
    pub columns: Vec<Column>,
    pub sort_by: Column,
}

impl Default for TableLayout {
    fn default() -> Self {
        Self {
            columns: DEFAULT_COLUMNS.to_vec(),
            sort_by: Column::Risk,
        }
    }
}

impl Column {
    fn header(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Version => "Version",
            Column::FixedIn => "Fixed in",
            Column::Kind => "Kind",
            Column::Source => "Source",
            Column::Vulnerability => "Vulnerability",
            Column::Severity => "Severity",
            Column::Epss => "EPSS",
            Column::Risk => "Risk",
            Column::Kev => "KEV",
            Column::Environments => "Environments",
            Column::Aliases => "Aliases",
            Column::Summary => "Summary",
            Column::Url => "URL",
            Column::Ignored => "Ignored",
            Column::Confidence => "Confidence",
        }
    }

    fn cell(self, vuln_match: &Match, row: RowContext) -> String {
        let dim_if_repeated = |text: String| {
            if row.repeated_package {
                style(text).dim().to_string()
            } else {
                text
            }
        };
        match self {
            Column::Name => dim_if_repeated(format!(
                "{} {}",
                kind_marker(&vuln_match.package.ecosystem),
                vuln_match.package.name
            )),
            Column::Version => dim_if_repeated(vuln_match.package.version.clone()),
            Column::FixedIn => match vuln_match.fix_availability() {
                FixAvailability::Available => style(vuln_match.fixed_versions.join(", "))
                    .green()
                    .to_string(),
                FixAvailability::ElsewhereOnly => {
                    style(fix_elsewhere_labels(vuln_match).join(", "))
                        .dim()
                        .to_string()
                }
                FixAvailability::None => String::new(),
            },
            Column::Kind => kind_name(&vuln_match.package.ecosystem),
            Column::Source => vuln_match.package.ecosystem.to_string(),
            Column::Vulnerability => match weak_confidence_label(vuln_match.match_confidence) {
                Some(label) if !row.confidence_column => {
                    format!("{} {}", vuln_match.id, style(format!("({label})")).dim())
                }
                _ => vuln_match.id.clone(),
            },
            Column::Severity => {
                let mut name = vuln_match.severity.name().to_string();
                if let Some(first) = name.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                severity_style(vuln_match.severity)
                    .apply_to(name)
                    .to_string()
            }
            Column::Epss => epss_label(&vuln_match.epss).unwrap_or_default(),
            Column::Risk => match (risk_label(vuln_match.risk), kev_label(&vuln_match.kev)) {
                (Some(risk), Some(kev)) => {
                    format!("{risk} {}", kev_style().apply_to(format!("({kev})")))
                }
                (Some(risk), None) => risk,
                (None, _) => String::new(),
            },
            Column::Kev => kev_label(&vuln_match.kev)
                .map(|label| kev_style().apply_to(label).to_string())
                .unwrap_or_default(),
            Column::Environments => consts::ENVIRONMENT_STYLE
                .apply_to(vuln_match.environments.join(", "))
                .to_string(),
            Column::Aliases => vuln_match.aliases.join(", "),
            Column::Summary => vuln_match.summary.clone().unwrap_or_default(),
            Column::Url => vuln_match.url.clone().unwrap_or_default(),
            Column::Ignored => match vuln_match.ignore_mode {
                Some(IgnoreMode::UntilFixed) => style("until a fix is released").dim().to_string(),
                Some(IgnoreMode::Permanent) => style("permanent").dim().to_string(),
                None => String::new(),
            },
            Column::Confidence => match confidence_label(vuln_match.match_confidence) {
                Some(label) if weak_confidence_label(vuln_match.match_confidence).is_some() => {
                    style(label).dim().to_string()
                }
                Some(label) => label.to_string(),
                None => String::new(),
            },
        }
    }

    /// The order two matches take when the table is sorted by this column.
    /// Risk, severity, EPSS, and KEV sort highest first; everything else
    /// ascends, ties keeping their existing order.
    fn compare(self, a: &Match, b: &Match) -> Ordering {
        match self {
            Column::Name => (a.package.name.as_str(), &a.package.version)
                .cmp(&(b.package.name.as_str(), &b.package.version)),
            Column::Version => a.package.version.cmp(&b.package.version),
            Column::FixedIn => match (a.fixed_versions.first(), b.fixed_versions.first()) {
                (Some(x), Some(y)) => x.cmp(y),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            Column::Kind => kind_name(&a.package.ecosystem).cmp(&kind_name(&b.package.ecosystem)),
            Column::Source => a.package.ecosystem.cmp(&b.package.ecosystem),
            Column::Vulnerability => a.id.cmp(&b.id),
            Column::Severity => SortKey::Severity.compare(a, b),
            Column::Epss => SortKey::Epss.compare(a, b),
            Column::Risk => SortKey::Risk.compare(a, b),
            Column::Kev => SortKey::Kev.compare(a, b),
            Column::Environments => a.environments.cmp(&b.environments),
            Column::Aliases => a.aliases.cmp(&b.aliases),
            Column::Summary => a.summary.cmp(&b.summary),
            Column::Url => a.url.cmp(&b.url),
            Column::Ignored => a.ignore_mode.is_some().cmp(&b.ignore_mode.is_some()),
            Column::Confidence => b.match_confidence.cmp(&a.match_confidence),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct RowContext {
    /// The previous row is the same locked package, so its identifying cells
    /// are dimmed.
    repeated_package: bool,
    /// The Confidence column is shown, so the Vulnerability cell need not
    /// flag weak matches itself.
    confidence_column: bool,
}

fn kind_name(ecosystem: &PackageEcosystem) -> String {
    match ecosystem {
        PackageEcosystem::Pypi => consts::PYPI_PACKAGE_STYLE.apply_to("pypi").to_string(),
        PackageEcosystem::CondaForge | PackageEcosystem::Other(_) => {
            consts::CONDA_PACKAGE_STYLE.apply_to("conda").to_string()
        }
    }
}

/// The kind marker `pixi lock` puts before a package name: a colored `C` or
/// `P` with colors, `(conda)` or `(pypi)` without.
fn kind_marker(ecosystem: &PackageEcosystem) -> String {
    let (letter, word, style) = match ecosystem {
        PackageEcosystem::Pypi => ("P", "pypi", &*consts::PYPI_PACKAGE_STYLE),
        PackageEcosystem::CondaForge | PackageEcosystem::Other(_) => {
            ("C", "conda", &*consts::CONDA_PACKAGE_STYLE)
        }
    };
    if console::colors_enabled() {
        style.apply_to(letter).bold().to_string()
    } else {
        format!("({word})")
    }
}

fn table(headers: impl IntoIterator<Item = &'static str>, rows: Vec<Vec<String>>) -> String {
    let mut table = Table::new();
    table
        .load_style(NOTHING)
        .set_content_arrangement(ContentArrangement::Disabled);
    let header_style = console::Style::new().bold().cyan();
    table.set_header(
        headers
            .into_iter()
            .map(|header| Cell::new(header_style.apply_to(header).to_string())),
    );
    for row in rows {
        table.add_row(row.into_iter().map(Cell::new));
    }
    table.lines().map(|line| line.trim().to_string()).join("\n")
}

fn matches_table(matches: &[Match], columns: &[Column], sort_by: Column) -> String {
    let mut sorted: Vec<&Match> = matches.iter().collect();
    sorted.sort_by(|a, b| sort_by.compare(a, b));
    let mut previous: Option<&Match> = None;
    let rows = sorted
        .into_iter()
        .map(|vuln_match| {
            let row = RowContext {
                repeated_package: previous.is_some_and(|p| p.package == vuln_match.package),
                confidence_column: columns.contains(&Column::Confidence),
            };
            previous = Some(vuln_match);
            columns
                .iter()
                .map(|column| column.cell(vuln_match, row))
                .collect()
        })
        .collect();
    table(columns.iter().map(|column| column.header()), rows)
}

fn not_audited_table(not_audited: &[AuditPackage]) -> String {
    let rows = not_audited
        .iter()
        .map(|package| {
            vec![
                format!("{} {}", kind_marker(&package.ecosystem), package.name),
                package.version.clone().unwrap_or_else(|| "?".to_string()),
                package.ecosystem.to_string(),
                consts::ENVIRONMENT_STYLE
                    .apply_to(package.environments.join(", "))
                    .to_string(),
            ]
        })
        .collect();
    table(["Name", "Version", "Source", "Environments"], rows)
}

fn status_block(report: &AuditReport, lock_file: &str) -> String {
    let summary = &report.summary;
    let check = style(console::Emoji("✔ ", "")).green();
    let packages = format!(
        "[{} audited{}]",
        super::count(summary.audited, "package", "packages"),
        if summary.not_audited > 0 {
            format!(", {} not audited", summary.not_audited)
        } else {
            String::new()
        }
    );
    let matches = match summary.vulnerable {
        0 => "[no vulnerability matches]".to_string(),
        1 => "[1 vulnerability match]".to_string(),
        n => format!("[{n} vulnerability matches]"),
    };
    let by_severity = SeverityRating::ALL
        .iter()
        .rev()
        .map(|rating| {
            let count = report
                .matches
                .iter()
                .filter(|m| m.severity == *rating)
                .count();
            let text = format!("{count} {}", rating.name());
            if count == 0 {
                style(text).dim().to_string()
            } else {
                severity_style(*rating).apply_to(text).to_string()
            }
        })
        .join(", ");
    let fixed = report
        .matches
        .iter()
        .filter(|m| m.fix_availability() == FixAvailability::Available)
        .count();
    let weak: String = [
        MatchConfidence::VersionUnbounded,
        MatchConfidence::Unevaluated,
    ]
    .into_iter()
    .filter_map(|confidence| {
        let count = report
            .matches
            .iter()
            .filter(|m| m.match_confidence == confidence)
            .count();
        (count > 0).then(|| {
            format!(
                ", {count} {}",
                confidence_label(confidence).unwrap_or_default()
            )
        })
    })
    .collect();
    let label = |text: String| format!("{text:<33}");
    let mut out = String::new();
    writeln!(
        out,
        "{check}{} {}",
        label(format!("Audited {lock_file}")),
        style(packages).dim()
    )
    .expect("writing to a string");
    writeln!(
        out,
        "{check}{} {}",
        label("Scanned for vulnerabilities".to_string()),
        style(matches).dim()
    )
    .expect("writing to a string");
    writeln!(out, "  {} {by_severity}", style("├── by severity:").dim())
        .expect("writing to a string");
    write!(
        out,
        "  {}",
        style(format!(
            "└── by status:   {fixed} fixed, {} not-fixed, {} ignored{weak}",
            report.matches.len() - fixed,
            summary.ignored
        ))
        .dim()
    )
    .expect("writing to a string");
    out
}

/// Renders the default report. Tables are laid out at their natural width,
/// never clipped: a narrow terminal soft-wraps long rows.
pub(in crate::audit) fn render_table(
    report: &AuditReport,
    lock_file: &str,
    layout: &TableLayout,
    options: GuidanceOptions,
) -> String {
    let mut sections = Vec::new();
    if !report.not_audited.is_empty() {
        sections.push(format!(
            "{}\n\n{}",
            style(format!(
                "Not audited ({}) — no vulnerability database for these sources",
                report.not_audited.len()
            ))
            .bold(),
            not_audited_table(&report.not_audited)
        ));
    }
    sections.push(status_block(report, lock_file));
    if !report.matches.is_empty() {
        sections.push(matches_table(
            &report.matches,
            &layout.columns,
            layout.sort_by,
        ));
    }
    if !report.ignored.is_empty() {
        let mut columns = layout.columns.clone();
        if !columns.contains(&Column::Ignored) {
            columns.push(Column::Ignored);
        }
        sections.push(format!(
            "{}\n\n{}",
            style(format!("Ignored ({})", report.ignored.len())).bold(),
            matches_table(&report.ignored, &columns, layout.sort_by)
        ));
    }
    sections.push(summary_line(report));
    let mut out = sections.join("\n\n");
    out.push('\n');
    render_guidance(&mut out, report, options);
    out
}

#[cfg(test)]
mod tests {
    use pixi_audit::{
        AuditSummary, CondaForgeBuild, MatchConfidence, PackageId, PackageName, RiskScore,
        UpstreamRelease,
    };

    use super::*;

    fn finding(name: PackageName, ecosystem: PackageEcosystem, id: &str) -> Match {
        Match {
            package: PackageId {
                name,
                version: "1.0".to_string(),
                ecosystem,
            },
            environments: vec!["default".to_string()],
            id: id.to_string(),
            aliases: vec![],
            severity: SeverityRating::High,
            cvss_base_score: None,
            fixed_versions: vec![],
            advisories: vec![],
            other_ecosystem_fixes: vec![],
            upstream_releases: vec![],
            match_confidence: MatchConfidence::Evaluated,
            matched_identity: None,
            summary: None,
            url: None,
            epss: vec![],
            kev: vec![],
            risk: RiskScore::NONE,
            ignore_mode: None,
        }
    }

    #[test]
    fn renders_every_section() {
        console::set_colors_enabled(false);
        let mut fixed = finding(
            PackageName::conda("openssl").expect("valid name"),
            PackageEcosystem::CondaForge,
            "CVE-1",
        );
        fixed.fixed_versions = vec!["1.1".to_string()];
        let mut elsewhere = finding(
            PackageName::conda("libpq").expect("valid name"),
            PackageEcosystem::CondaForge,
            "CVE-2",
        );
        elsewhere.severity = SeverityRating::Unknown;
        elsewhere.match_confidence = MatchConfidence::VersionUnbounded;
        elsewhere.upstream_releases = vec![UpstreamRelease {
            version: "1.2".to_string(),
            conda_forge_build: CondaForgeBuild::Missing,
        }];
        let mut ignored = finding(
            PackageName::pypi("requests").expect("valid name"),
            PackageEcosystem::Pypi,
            "GHSA-1",
        );
        ignored.ignore_mode = Some(IgnoreMode::UntilFixed);
        let report = AuditReport {
            matches: vec![fixed, elsewhere],
            ignored: vec![ignored],
            not_audited: vec![AuditPackage {
                name: PackageName::conda("samtools").expect("valid name"),
                version: Some("1.19".to_string()),
                ecosystem: PackageEcosystem::Other("bioconda".to_string()),
                environments: vec!["default".to_string()],
            }],
            summary: AuditSummary {
                audited: 3,
                vulnerable: 2,
                ignored: 1,
                not_audited: 1,
            },
            ..AuditReport::default()
        };
        insta::assert_snapshot!(
            render_table(&report, "pixi.lock", &TableLayout::default(), GuidanceOptions::default()),
            @"
        Not audited (1) — no vulnerability database for these sources

        Name              Version  Source    Environments
        (conda) samtools  1.19     bioconda  default

        ✔ Audited pixi.lock                 [3 packages audited, 1 not audited]
        ✔ Scanned for vulnerabilities       [2 vulnerability matches]
          ├── by severity: 0 critical, 1 high, 0 medium, 0 low, 1 unknown
          └── by status:   1 fixed, 1 not-fixed, 1 ignored, 1 no version limits

        Name             Version  Fixed in       Vulnerability              Severity  EPSS  Risk  Environments
        (conda) openssl  1.0      1.1            CVE-1                      High                  default
        (conda) libpq    1.0      upstream: 1.2  CVE-2 (no version limits)  Unknown               default

        Ignored (1)

        Name             Version  Fixed in  Vulnerability  Severity  EPSS  Risk  Environments  Ignored
        (pypi) requests  1.0                GHSA-1         High                  default       until a fix is released

        ✘ 2 vulnerabilities in 2 packages (3 audited, 1 ignored, 1 not audited)

        Overview
          • Highest risk score:                    n/a
          • CISA KEV matches:                      n/a — CISA KEV catalog not checked
          • Critical/high vulnerability matches:   1
          • Fix available in audited ecosystem:    1 of 2 vulnerabilities across 1 audited package
          • Fix only elsewhere:                    1 vulnerability
          • No known fix:                          0 vulnerabilities

        Recommendations
          • Re-run this audit with `--fix` to update the lock file to fixed releases. Fixed releases in the audited ecosystem are reported for 1 vulnerability.
          • 1 vulnerability has a fix only elsewhere. `--fix` cannot install it. Wait for an ecosystem build or change the dependency source.
          • Review 1 package that was not audited. Its source is not covered by the configured vulnerability databases.

        Add `--explain` for scoring and report definitions.
        "
        );
    }
}
