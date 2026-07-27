use std::collections::BTreeMap;

use clap::Parser;
use miette::{Context, IntoDiagnostic};
use pixi_audit::{
    AuditPackage, AuditReport, BASE_URL_ENV_VAR, BasiliskClient, DEFAULT_BASE_URL,
    PackageEcosystem, SeverityBand,
};
use pixi_core::{WorkspaceLocator, lock_file::UpdateLockFileOptions};
use pixi_manifest::HasWorkspaceManifest;
use pixi_utils::reqwest::build_reqwest_clients;
use rattler_lock::{LockFile, LockedPackage};
use url::Url;

use crate::cli_config::{LockFileUpdateConfig, NoInstallConfig, WorkspaceConfig};

/// Audit the workspace's locked packages for known vulnerabilities.
///
/// Queries the basilisk vulnerability database (an OSV-compatible API) for
/// every package in the lock file, across all environments and platforms.
/// Exits with a non-zero status code when a vulnerability is found that is
/// not ignored via `[workspace.audit] ignore` in the manifest.
#[derive(Debug, Parser)]
pub struct Args {
    #[clap(flatten)]
    pub config_source: pixi_config::ConfigSourceCli,

    #[clap(flatten)]
    pub workspace_config: WorkspaceConfig,

    #[clap(flatten)]
    pub lock_file_update_config: LockFileUpdateConfig,

    #[clap(flatten)]
    pub no_install_config: NoInstallConfig,

    /// The environment(s) to audit. Defaults to all environments in the
    /// lock file.
    #[arg(short, long = "environment")]
    pub environments: Vec<String>,

    /// The platform(s) to audit. Defaults to all platforms in the lock file.
    #[arg(long = "platform")]
    pub platforms: Vec<String>,

    /// Whether to output in json format.
    #[arg(long)]
    pub json: bool,
}

pub async fn execute(args: Args) -> miette::Result<()> {
    let workspace = WorkspaceLocator::for_cli()
        .with_global_config_source(args.config_source.source())
        .with_search_start(args.workspace_config.workspace_locator_start())
        .locate()?;

    let lock_file = workspace
        .update_lock_file(
            Some(pixi_reporters::TopLevelProgress::from_global()),
            UpdateLockFileOptions {
                lock_file_usage: args.lock_file_update_config.lock_file_usage()?,
                no_install: args.no_install_config.no_install,
                max_concurrent_solves: workspace.config().max_concurrent_solves(),
                ..Default::default()
            },
        )
        .await
        .wrap_err("Failed to update lock file")?
        .0
        .into_lock_file();

    let packages = collect_packages(&lock_file, &args.environments, &args.platforms)?;

    let ignore = (&workspace)
        .workspace_manifest()
        .workspace
        .audit
        .clone()
        .unwrap_or_default()
        .ignore;

    let base_url = match std::env::var(BASE_URL_ENV_VAR) {
        Ok(value) => Url::parse(&value)
            .into_diagnostic()
            .wrap_err_with(|| format!("Invalid URL in {BASE_URL_ENV_VAR}"))?,
        Err(_) => Url::parse(DEFAULT_BASE_URL).expect("default base URL is valid"),
    };
    let (_, client) = build_reqwest_clients(Some(workspace.config()), None)?;
    let client = BasiliskClient::new(client, base_url);

    eprintln!("Auditing {} packages...", packages.len());
    let report = pixi_audit::audit(&client, packages, &ignore).await?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).into_diagnostic()?
        );
    } else {
        print!("{}", format_report(&report));
    }

    if !report.vulnerabilities.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

/// Classifies a conda channel URL: conda-forge is the only ecosystem the
/// vulnerability database can currently answer for.
fn classify_channel(channel_url: Option<&str>) -> PackageEcosystem {
    match channel_url {
        Some(url) => {
            let trimmed = url.trim_end_matches('/');
            if trimmed.ends_with("/conda-forge") {
                PackageEcosystem::CondaForge
            } else {
                let name = trimmed.rsplit('/').next().unwrap_or("unknown");
                PackageEcosystem::Other(name.to_string())
            }
        }
        None => PackageEcosystem::Other("unknown".to_string()),
    }
}

/// Collects deduplicated `(name, version, ecosystem)` packages from the lock
/// file with the set of environments containing each.
fn collect_packages(
    lock_file: &LockFile,
    environment_filter: &[String],
    platform_filter: &[String],
) -> miette::Result<Vec<AuditPackage>> {
    if lock_file.environments().len() == 0 {
        miette::bail!("the lock file contains no environments; run `pixi lock` first",);
    }

    // (name, version, ecosystem) -> environment names
    let mut dedup: BTreeMap<
        (String, String, String),
        (PackageEcosystem, std::collections::BTreeSet<String>),
    > = BTreeMap::new();

    for (environment_name, environment) in lock_file.environments() {
        if !environment_filter.is_empty()
            && !environment_filter.iter().any(|e| e == environment_name)
        {
            continue;
        }
        for (platform, packages) in environment.packages_by_platform() {
            if !platform_filter.is_empty()
                && !platform_filter
                    .iter()
                    .any(|p| p.as_str() == platform.name().as_str())
            {
                continue;
            }
            for package in packages {
                let (name, version, ecosystem) = match package {
                    LockedPackage::Conda(conda) => {
                        let record = conda.record();
                        let channel = conda
                            .as_binary()
                            .and_then(|binary| binary.channel.as_ref())
                            .map(|channel| channel.to_string());
                        (
                            conda.name().as_normalized().to_string(),
                            record.map(|r| r.version.to_string()).unwrap_or_default(),
                            classify_channel(channel.as_deref()),
                        )
                    }
                    LockedPackage::Pypi(pypi) => (
                        pypi.name().to_string(),
                        pypi.version_string(),
                        PackageEcosystem::Pypi,
                    ),
                };
                let key = (name, version, ecosystem.osv_ecosystem().to_string());
                dedup
                    .entry(key)
                    .or_insert_with(|| (ecosystem.clone(), Default::default()))
                    .1
                    .insert(environment_name.to_string());
            }
        }
    }

    Ok(dedup
        .into_iter()
        .map(
            |((name, version, _), (ecosystem, environments))| AuditPackage {
                name,
                version,
                ecosystem,
                environments: environments.into_iter().collect(),
            },
        )
        .collect())
}

/// Renders the human-readable report.
fn format_report(report: &AuditReport) -> String {
    use std::fmt::Write;

    let mut out = String::new();
    if !report.vulnerabilities.is_empty() {
        let _ = writeln!(
            out,
            "{:<20} {:<15} {:<18} {:<10} {:<15} Environments",
            "Package", "Version", "Vulnerability", "Severity", "Fixed in"
        );
        for finding in &report.vulnerabilities {
            let fixed = if finding.fixed_versions.is_empty() {
                "-".to_string()
            } else {
                finding.fixed_versions.join(", ")
            };
            let _ = writeln!(
                out,
                "{:<20} {:<15} {:<18} {:<10} {:<15} {}",
                finding.package,
                finding.version,
                finding.id,
                // `SeverityBand::fmt` writes via `write_str`, which does not
                // honor the formatter's width/alignment; pad the rendered
                // string explicitly so the column stays aligned.
                finding.severity.to_string(),
                fixed,
                finding.environments.join(", "),
            );
            if !finding.aliases.is_empty() {
                let _ = writeln!(out, "    aliases: {}", finding.aliases.join(", "));
            }
            if let Some(url) = &finding.url {
                let _ = writeln!(out, "    more info: {url}");
            }
        }
        let _ = writeln!(out);
    }

    // Summary lines, always shown.
    let mut by_band: BTreeMap<SeverityBand, usize> = BTreeMap::new();
    for finding in &report.vulnerabilities {
        *by_band.entry(finding.severity).or_default() += 1;
    }
    let breakdown = if by_band.is_empty() {
        String::new()
    } else {
        let parts: Vec<String> = by_band
            .iter()
            .rev()
            .map(|(band, count)| format!("{count} {band}"))
            .collect();
        format!(" ({})", parts.join(", "))
    };
    let _ = writeln!(
        out,
        "Audited {} packages: {} vulnerabilities found{}",
        report.summary.audited, report.summary.vulnerable, breakdown
    );
    if report.summary.unchecked > 0 {
        let mut by_ecosystem: BTreeMap<&str, usize> = BTreeMap::new();
        for unchecked in &report.unchecked {
            *by_ecosystem
                .entry(unchecked.ecosystem.as_str())
                .or_default() += 1;
        }
        let parts: Vec<String> = by_ecosystem
            .iter()
            .map(|(eco, count)| format!("{count} {eco}"))
            .collect();
        let _ = writeln!(
            out,
            "Not checked: {} packages ({})",
            report.summary.unchecked,
            parts.join(", ")
        );
    }
    if report.summary.ignored > 0 {
        let _ = writeln!(
            out,
            "Ignored: {} findings suppressed via [workspace.audit] in the manifest",
            report.summary.ignored
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use pixi_audit::{
        AuditReport, AuditSummary, Finding, PackageEcosystem, SeverityBand, UncheckedPackage,
    };

    use super::*;

    #[test]
    fn classify_channels() {
        assert_eq!(
            classify_channel(Some("https://conda.anaconda.org/conda-forge/")),
            PackageEcosystem::CondaForge
        );
        assert_eq!(
            classify_channel(Some("https://prefix.dev/conda-forge")),
            PackageEcosystem::CondaForge
        );
        assert_eq!(
            classify_channel(Some("https://conda.anaconda.org/bioconda/")),
            PackageEcosystem::Other("bioconda".to_string())
        );
        assert_eq!(
            classify_channel(None),
            PackageEcosystem::Other("unknown".to_string())
        );
    }

    fn sample_report() -> AuditReport {
        AuditReport {
            vulnerabilities: vec![Finding {
                package: "openssl".to_string(),
                version: "3.1.0".to_string(),
                ecosystem: "conda-forge".to_string(),
                environments: vec!["default".to_string(), "prod".to_string()],
                id: "BSLK-1".to_string(),
                aliases: vec!["CVE-2026-1234".to_string()],
                severity: SeverityBand::Critical,
                fixed_versions: vec!["3.1.1".to_string()],
                summary: Some("Buffer overflow".to_string()),
                url: Some("https://example.com/adv".to_string()),
            }],
            ignored: vec![],
            unchecked: vec![UncheckedPackage {
                package: "requests".to_string(),
                version: "2.32.0".to_string(),
                ecosystem: "PyPI".to_string(),
                environments: vec!["default".to_string()],
            }],
            summary: AuditSummary {
                audited: 2,
                vulnerable: 1,
                ignored: 0,
                unchecked: 1,
            },
        }
    }

    #[test]
    fn render_human_report() {
        insta::assert_snapshot!(format_report(&sample_report()));
    }

    #[test]
    fn render_clean_report() {
        let report = AuditReport {
            summary: AuditSummary {
                audited: 5,
                ..Default::default()
            },
            ..Default::default()
        };
        insta::assert_snapshot!(format_report(&report));
    }
}
