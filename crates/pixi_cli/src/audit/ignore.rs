//! `pixi audit ignore`: records ignore rules in `[workspace.audit]`, so an
//! exception lives next to the workspace it covers instead of in a CI script.

use chrono::NaiveDate;
use clap::Parser;
use pixi_core::WorkspaceLocator;
use pixi_manifest::{AuditEcosystem, AuditIgnoreList, AuditIgnoreRule};

/// Record ignore rules in the `[workspace.audit]` table of the manifest.
#[derive(Debug, Parser)]
pub struct Args {
    /// Advisory ids or aliases to ignore. With `--package`, only in that
    /// package; without any id, `--package` ignores every advisory of the
    /// package.
    #[arg(required_unless_present = "package", value_name = "ID")]
    pub ids: Vec<String>,

    /// Limit the rule to this package, or ignore every advisory of it.
    #[arg(long, value_name = "NAME")]
    pub package: Option<String>,

    /// Limit the rule to this ecosystem (`conda-forge` or `PyPI`).
    #[arg(long, value_name = "ECOSYSTEM", value_parser = parse_ecosystem)]
    pub ecosystem: Option<AuditEcosystem>,

    /// Limit the rule to this locked version.
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,

    /// Limit the rule to locked versions up to and including this one.
    #[arg(long, value_name = "VERSION")]
    pub up_to_version: Option<String>,

    /// Ignore the advisories only while no fixed release newer than the
    /// locked version is reported in the
    /// package's own ecosystem. Requires advisory ids.
    #[arg(long, requires = "ids")]
    pub until_fixed: bool,

    /// Why the advisories are ignored.
    #[arg(long, value_name = "TEXT")]
    pub reason: Option<String>,

    /// The last day, in UTC, on which the rule applies (`YYYY-MM-DD`).
    #[arg(long, value_name = "DATE", value_parser = parse_date)]
    pub expired_at: Option<NaiveDate>,
}

fn parse_ecosystem(value: &str) -> Result<AuditEcosystem, String> {
    value.parse()
}

fn parse_date(value: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|err| format!("'{value}' is not a date (expected YYYY-MM-DD): {err}"))
}

pub(super) async fn execute(args: Args, locator: WorkspaceLocator) -> miette::Result<()> {
    if let Some(expired_at) = args.expired_at
        && expired_at < super::policy::today()
    {
        miette::bail!("--expired-at {expired_at} is in the past");
    }
    let workspace = locator.locate().await?;

    let list = if args.until_fixed {
        AuditIgnoreList::IgnoreUntilFixed
    } else {
        AuditIgnoreList::Ignore
    };
    let rule = |id: Option<String>| AuditIgnoreRule {
        id,
        package: args.package.clone(),
        ecosystem: args.ecosystem,
        version: args.version.clone(),
        up_to_version: args.up_to_version.clone(),
        reason: args.reason.clone(),
        expired_at: args.expired_at,
    };
    let rules = if args.ids.is_empty() {
        vec![rule(None)]
    } else {
        args.ids.iter().cloned().map(Some).map(rule).collect()
    };

    let mut workspace = workspace.modify()?;
    let outcome = workspace.manifest().add_audit_ignore_rules(list, rules)?;
    let manifest_path = workspace.workspace().workspace.provenance.path.clone();
    if !outcome.added.is_empty() {
        workspace.save().await.map_err(|source| {
            super::error::AuditCommandError::SaveManifest(manifest_path.clone(), source)
        })?;
    }

    for rule in &outcome.added {
        eprintln!(
            "{}Ignoring {rule}{} in {}",
            console::style(console::Emoji("✔ ", "")).green(),
            match list {
                AuditIgnoreList::Ignore => "",
                AuditIgnoreList::IgnoreUntilFixed => " until a fix is released",
            },
            manifest_path.display()
        );
    }
    for rule in &outcome.existing {
        eprintln!(
            "{}{rule} is already listed in `{}`; its reason and expiry are unchanged",
            console::style(console::Emoji("i ", "i ")).blue(),
            list.key(),
        );
    }
    Ok(())
}
