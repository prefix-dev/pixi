//! SARIF 2.1.0 output, the format GitHub code scanning and similar tools
//! ingest. One rule per vulnerability and package, one result per
//! vulnerability match, located at the package's line in the lock file.
//! Ignored vulnerability matches are results with a suppression, so
//! dashboards can show them as dismissed rather than losing them.

use std::collections::BTreeSet;
use std::path::Path;

use pixi_audit::{AuditReport, IgnoreMode, Match, PackageEcosystem, SeverityRating};
use serde::Serialize;

use super::{
    confidence_label, confidence_note, epss_probability_first_label, fix_summary, kev_label,
    risk_label,
};

/// The lock file a report was produced from: the path written into the
/// log, and the contents used to find each package's line.
pub(in crate::audit) struct LockFileSource<'a> {
    pub path: &'a Path,
    pub contents: &'a str,
}

/// A SARIF log with a single run.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::audit) struct SarifLog {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: Vec<Run>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Run {
    tool: Tool,
    invocations: Vec<Invocation>,
    results: Vec<SarifResult>,
}

/// The audit ran to completion; a failed audit never produces a log.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Invocation {
    execution_successful: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Tool {
    driver: Driver,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Driver {
    name: &'static str,
    version: &'static str,
    semantic_version: &'static str,
    information_uri: &'static str,
    rules: Vec<Rule>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Rule {
    id: String,
    name: String,
    short_description: Message,
    full_description: Message,
    #[serde(skip_serializing_if = "Option::is_none")]
    help_uri: Option<String>,
    help: MultiformatMessage,
    properties: RuleProperties,
}

/// The properties GitHub reads from a rule: `security-severity` drives the
/// alert's severity label, tags its category filter.
#[derive(Debug, Serialize)]
struct RuleProperties {
    #[serde(rename = "security-severity")]
    security_severity: String,
    tags: Vec<String>,
    purls: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Message {
    text: String,
}

#[derive(Debug, Serialize)]
struct MultiformatMessage {
    text: String,
    markdown: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult {
    rule_id: String,
    kind: &'static str,
    level: &'static str,
    message: Message,
    locations: Vec<Location>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    suppressions: Vec<Suppression>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    physical_location: PhysicalLocation,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    artifact_location: ArtifactLocation,
    region: Region,
}

#[derive(Debug, Serialize)]
struct ArtifactLocation {
    uri: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    start_line: usize,
}

#[derive(Debug, Serialize)]
struct Suppression {
    kind: &'static str,
    justification: String,
}

impl SarifLog {
    pub(in crate::audit) fn from_report(
        report: &AuditReport,
        lock_file: &LockFileSource<'_>,
    ) -> Self {
        let matches = report.matches.iter().chain(&report.ignored);
        let mut seen = BTreeSet::new();
        let rules = matches
            .clone()
            .filter(|m| seen.insert(rule_id(m)))
            .map(|m| rule(m, lock_file))
            .collect();
        let uri = artifact_uri(lock_file.path);
        let results = matches
            .map(|m| result(m, &uri, lock_file.contents))
            .collect();
        Self {
            schema: "https://json.schemastore.org/sarif-2.1.0.json",
            version: "2.1.0",
            runs: vec![Run {
                tool: Tool {
                    driver: Driver {
                        name: "pixi",
                        version: pixi_consts::consts::PIXI_VERSION,
                        semantic_version: pixi_consts::consts::PIXI_VERSION,
                        information_uri: "https://pixi.sh/latest/reference/cli/pixi/audit/",
                        rules,
                    },
                },
                invocations: vec![Invocation {
                    execution_successful: true,
                }],
                results,
            }],
        }
    }
}

/// One rule per vulnerability and package, so dashboards group alerts the
/// way people fix them.
fn rule_id(vuln_match: &Match) -> String {
    format!("{}-{}", vuln_match.id, vuln_match.package.name)
}

fn rule(vuln_match: &Match, lock_file: &LockFileSource<'_>) -> Rule {
    let link = match &vuln_match.url {
        Some(url) => format!("[{}]({url})", vuln_match.id),
        None => vuln_match.id.clone(),
    };
    let rows: Vec<(&str, String)> = vec![
        ("Severity", vuln_match.severity.name().to_string()),
        (
            "CVSS",
            vuln_match
                .cvss_base_score
                .map(|score| format!("{score:.1}"))
                .unwrap_or_default(),
        ),
        (
            "EPSS",
            epss_probability_first_label(&vuln_match.epss).unwrap_or_default(),
        ),
        ("Risk", risk_label(vuln_match.risk).unwrap_or_default()),
        (
            "KEV",
            kev_label(&vuln_match.kev)
                .map(str::to_string)
                .unwrap_or_default(),
        ),
        ("Package", vuln_match.package.name.to_string()),
        ("Version", vuln_match.package.version.clone()),
        ("Ecosystem", vuln_match.package.ecosystem.to_string()),
        ("Fix", fix_summary(vuln_match)),
        (
            "Match confidence",
            confidence_label(vuln_match.match_confidence)
                .map(str::to_string)
                .unwrap_or_default(),
        ),
        (
            "Matched identity",
            vuln_match
                .matched_identity
                .as_ref()
                .map(|identity| identity.value.clone())
                .unwrap_or_default(),
        ),
        ("Environments", vuln_match.environments.join(", ")),
        ("Lock file", artifact_uri(lock_file.path)),
        ("Link", link),
    ];
    let text = std::iter::once(format!("Vulnerability {}", vuln_match.id))
        .chain(
            rows.iter()
                .map(|(label, value)| format!("{label}: {value}")),
        )
        .collect::<Vec<_>>()
        .join("\n");
    let markdown = format!(
        "**Vulnerability {}**\n| {} |\n| {} |\n| {} |\n",
        vuln_match.id,
        rows.iter()
            .map(|(label, _)| *label)
            .collect::<Vec<_>>()
            .join(" | "),
        rows.iter().map(|_| "---").collect::<Vec<_>>().join(" | "),
        rows.iter()
            .map(|(_, value)| value.replace('|', "\\|"))
            .collect::<Vec<_>>()
            .join(" | "),
    );
    Rule {
        id: rule_id(vuln_match),
        name: vuln_match.id.clone(),
        short_description: Message {
            text: format!(
                "{} {} vulnerability for {} package",
                vuln_match.id,
                vuln_match.severity.name(),
                vuln_match.package.name
            ),
        },
        full_description: Message {
            text: vuln_match.summary.clone().unwrap_or_else(|| {
                format!("{} affects {}", vuln_match.id, vuln_match.package.name)
            }),
        },
        help_uri: vuln_match.url.clone(),
        help: MultiformatMessage { text, markdown },
        properties: RuleProperties {
            security_severity: security_severity(vuln_match),
            tags: vec![
                "security".to_string(),
                vuln_match.severity.name().to_string(),
            ],
            purls: vec![purl(vuln_match)],
        },
    }
}

/// GitHub maps `security-severity` onto its labels with CVSS thresholds
/// (critical >= 9.0, high >= 7.0, medium >= 4.0), so the CVSS base score
/// is used when known and the rating's lower bound otherwise.
fn security_severity(vuln_match: &Match) -> String {
    match vuln_match.cvss_base_score {
        Some(score) => format!("{score:.1}"),
        None => match vuln_match.severity {
            SeverityRating::Critical => "9.0",
            SeverityRating::High => "7.0",
            SeverityRating::Medium => "4.0",
            SeverityRating::Low => "1.0",
            SeverityRating::Unknown => "0.0",
        }
        .to_string(),
    }
}

/// The result level, following grype: critical and high are errors,
/// medium a warning, the rest notes.
fn level(rating: SeverityRating) -> &'static str {
    match rating {
        SeverityRating::Critical | SeverityRating::High => "error",
        SeverityRating::Medium => "warning",
        SeverityRating::Low | SeverityRating::Unknown => "note",
    }
}

fn purl(vuln_match: &Match) -> String {
    let name = vuln_match.package.name.as_str();
    let version = &vuln_match.package.version;
    match &vuln_match.package.ecosystem {
        PackageEcosystem::Pypi => format!("pkg:pypi/{name}@{version}"),
        PackageEcosystem::CondaForge => format!("pkg:conda/{name}@{version}?channel=conda-forge"),
        PackageEcosystem::Other(channel) => {
            format!("pkg:conda/{name}@{version}?channel={channel}")
        }
    }
}

fn result(vuln_match: &Match, uri: &str, lock_file_contents: &str) -> SarifResult {
    let suppressions = match vuln_match.ignore_mode {
        Some(IgnoreMode::Permanent) => vec![Suppression {
            kind: "external",
            justification: "ignored by an audit ignore rule".to_string(),
        }],
        Some(IgnoreMode::UntilFixed) => vec![Suppression {
            kind: "external",
            justification: "ignored until a fix is released".to_string(),
        }],
        None => Vec::new(),
    };
    SarifResult {
        rule_id: rule_id(vuln_match),
        kind: "fail",
        level: level(vuln_match.severity),
        message: Message {
            text: format!(
                "{}: {} vulnerability in {} package {} {}. {}{}",
                vuln_match.id,
                vuln_match.severity.name(),
                vuln_match.package.ecosystem,
                vuln_match.package.name,
                vuln_match.package.version,
                fix_summary(vuln_match),
                confidence_note(vuln_match)
                    .map(|note| format!(". {note}"))
                    .unwrap_or_default()
            ),
        },
        locations: vec![Location {
            physical_location: PhysicalLocation {
                artifact_location: ArtifactLocation {
                    uri: uri.to_string(),
                },
                region: Region {
                    start_line: package_line(lock_file_contents, vuln_match),
                },
            },
        }],
        suppressions,
    }
}

/// The lock file path as code scanning expects it: relative to the git
/// repository containing it, else to the working directory, else as is.
/// Forward slashes either way.
fn artifact_uri(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let relative_to_root = git_repository_root(&absolute)
        .and_then(|root| absolute.strip_prefix(root).ok().map(Path::to_path_buf));
    let relative_to_cwd = || {
        std::env::current_dir()
            .ok()
            .and_then(|cwd| absolute.strip_prefix(cwd).ok().map(Path::to_path_buf))
    };
    relative_to_root
        .or_else(relative_to_cwd)
        .unwrap_or_else(|| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

/// The nearest ancestor of `path` holding a `.git` directory, or the
/// `.git` file of a worktree or submodule.
fn git_repository_root(path: &Path) -> Option<&Path> {
    path.ancestors()
        .skip(1)
        .find(|ancestor| ancestor.join(".git").exists())
}

/// The first line of the lock file naming the package at its version: the
/// archive filename in a package URL, such as `openssl-3.1.0-hd590300_3`
/// or `requests-2.32.0-py3-none-any.whl`. Line 1 when nothing matches.
fn package_line(contents: &str, vuln_match: &Match) -> usize {
    let version = vuln_match.package.version.to_ascii_lowercase();
    let name = vuln_match.package.name.as_str().to_ascii_lowercase();
    // Wheel and sdist filenames spell the name with underscores.
    let spellings = [name.clone(), name.replace('-', "_")];
    let needles: Vec<String> = spellings
        .iter()
        .flat_map(|spelling| {
            [
                format!("/{spelling}-{version}-"),
                format!("/{spelling}-{version}."),
            ]
        })
        .collect();
    contents
        .lines()
        .position(|line| {
            let line = line.to_ascii_lowercase();
            needles.iter().any(|needle| line.contains(needle.as_str()))
        })
        .map_or(1, |index| index + 1)
}
