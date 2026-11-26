//! Combines the `[workspace.audit]` policy with the command line into the
//! effective policy of one invocation. Normal audits, remediation target
//! selection, and candidate acceptance all use the result, so they agree on
//! which vulnerability matches fail.

use chrono::NaiveDate;
use pixi_audit::{IgnoreMode, IgnoreRule, IgnoreScope, PackageEcosystem, Policy, SeverityRating};
use pixi_manifest::{AuditIgnoreList, AuditIgnoreRule, AuditPolicy, AuditSeverity};

/// An ignore rule whose last day has passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExpiredRule {
    pub rule: AuditIgnoreRule,
    pub expired_at: NaiveDate,
}

/// The policy of one invocation.
#[derive(Debug, Clone)]
pub(super) struct EffectivePolicy {
    pub rules: Vec<IgnoreRule>,
    pub policy: Policy,
    pub expired: Vec<ExpiredRule>,
}

/// The policy options given on the command line.
#[derive(Debug, Clone, Default)]
pub(super) struct CliPolicy {
    pub ignore: Vec<String>,
    pub ignore_until_fixed: Vec<String>,
    pub fail_on: Option<AuditSeverity>,
    pub warn_only: Vec<PackageEcosystem>,
}

/// Today in UTC, the reference date for rule expiry.
pub(super) fn today() -> NaiveDate {
    chrono::Utc::now().date_naive()
}

fn severity(rating: AuditSeverity) -> SeverityRating {
    match rating {
        AuditSeverity::Low => SeverityRating::Low,
        AuditSeverity::Medium => SeverityRating::Medium,
        AuditSeverity::High => SeverityRating::High,
        AuditSeverity::Critical => SeverityRating::Critical,
    }
}

fn engine_rule(rule: &AuditIgnoreRule, mode: IgnoreMode) -> miette::Result<IgnoreRule> {
    IgnoreRule::new(
        rule.id.clone(),
        IgnoreScope {
            package: rule.package.clone(),
            ecosystem: rule
                .ecosystem
                .map(|ecosystem| ecosystem.as_str().to_string()),
            version: rule.version.clone(),
            up_to_version: rule.up_to_version.clone(),
        },
        mode,
    )
    .map_err(|err| miette::miette!("invalid ignore rule {rule}: {err}"))
}

/// Resolves the manifest policy and the command line against `today`. A rule
/// applies through its `expired-at` day inclusive. Command-line ignores are
/// added to the manifest's, and a command-line threshold replaces the
/// manifest's.
pub(super) fn resolve(
    manifest: &AuditPolicy,
    cli: &CliPolicy,
    today: NaiveDate,
) -> miette::Result<EffectivePolicy> {
    let mut rules = Vec::new();
    let mut expired = Vec::new();
    for (list, mode) in [
        (AuditIgnoreList::Ignore, IgnoreMode::Permanent),
        (AuditIgnoreList::IgnoreUntilFixed, IgnoreMode::UntilFixed),
    ] {
        for rule in manifest.rules(list) {
            match rule.expired_at {
                Some(expired_at) if expired_at < today => expired.push(ExpiredRule {
                    rule: rule.clone(),
                    expired_at,
                }),
                Some(_) | None => rules.push(engine_rule(rule, mode)?),
            }
        }
    }
    rules.extend(
        cli.ignore
            .iter()
            .map(|id| IgnoreRule::from_id(id.clone(), IgnoreMode::Permanent)),
    );
    rules.extend(
        cli.ignore_until_fixed
            .iter()
            .map(|id| IgnoreRule::from_id(id.clone(), IgnoreMode::UntilFixed)),
    );
    Ok(EffectivePolicy {
        rules,
        policy: Policy {
            fail_on: cli.fail_on.or(manifest.fail_on).map(severity),
            warn_only: cli.warn_only.clone(),
        },
        expired,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_apply_through_their_last_day_and_the_cli_adds_to_the_manifest() {
        let date = |day| NaiveDate::from_ymd_opt(2026, 11, day).expect("valid date");
        let manifest = AuditPolicy {
            fail_on: Some(AuditSeverity::Medium),
            ignore: vec![
                AuditIgnoreRule {
                    expired_at: Some(date(26)),
                    ..AuditIgnoreRule::from_id("CVE-TODAY")
                },
                AuditIgnoreRule {
                    expired_at: Some(date(25)),
                    ..AuditIgnoreRule::from_id("CVE-YESTERDAY")
                },
            ],
            ignore_until_fixed: vec![AuditIgnoreRule::from_id("CVE-UNFIXED")],
        };
        let cli = CliPolicy {
            ignore: vec!["CVE-CLI".to_string()],
            fail_on: Some(AuditSeverity::Critical),
            ..CliPolicy::default()
        };
        let effective = resolve(&manifest, &cli, date(26)).expect("valid policy");
        let rules: Vec<_> = effective
            .rules
            .iter()
            .map(|rule| (rule.id().expect("id"), rule.mode()))
            .collect();
        assert_eq!(
            rules,
            [
                ("CVE-TODAY", IgnoreMode::Permanent),
                ("CVE-UNFIXED", IgnoreMode::UntilFixed),
                ("CVE-CLI", IgnoreMode::Permanent),
            ]
        );
        assert_eq!(effective.expired.len(), 1);
        assert_eq!(effective.expired[0].expired_at, date(25));
        assert_eq!(effective.policy.fail_on, Some(SeverityRating::Critical));
    }
}
