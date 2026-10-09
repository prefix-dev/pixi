use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use rattler_conda_types::{ChannelUrl, MatchSpec, PackageName};
pub use rattler_solve::InvalidExemptionError;
use std::{collections::BTreeMap, str::FromStr};

/// Specifies how to exclude newer packages from the solve.
///
/// Can be either:
/// - An absolute timestamp
/// - A relative duration (e.g., `7d`, `1h`, `30m`, `1h30m`)
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum ExcludeNewer {
    /// An absolute point in time. Packages newer than this are excluded.
    Timestamp(DateTime<Utc>),
    /// A relative duration. At solve time, packages newer than `now - duration`
    /// are excluded.
    Duration(std::time::Duration),
}

/// A fully resolved exclude-newer configuration with absolute cutoffs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResolvedExcludeNewer {
    /// The default cutoff date. Packages uploaded after this date are excluded.
    pub cutoff: DateTime<Utc>,

    /// Channel-specific cutoff dates that override [`Self::cutoff`] for
    /// records from matching channels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub channel_cutoffs: BTreeMap<ChannelUrl, DateTime<Utc>>,

    /// Package-specific cutoff dates that override both [`Self::cutoff`] and
    /// [`Self::channel_cutoffs`] for matching package names.
    ///
    /// Deprecated in favor of [`Self::exemptions`]. Only the deprecated
    /// top-level `[exclude-newer]` manifest table still populates this.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub package_cutoffs: BTreeMap<PackageName, DateTime<Utc>>,

    /// Records matching any of these specs are never excluded, regardless of
    /// their timestamp. This allows a single vetted release through without
    /// lowering the cutoff for every future release of the package.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exemptions: Vec<MatchSpec>,

    /// Whether to include packages that don't have a timestamp.
    pub include_unknown_timestamp: bool,
}

impl ExcludeNewer {
    /// Returns the effective cutoff for the current time.
    pub fn cutoff(&self) -> DateTime<Utc> {
        match self {
            Self::Timestamp(cutoff) => *cutoff,
            Self::Duration(duration) => {
                let duration = chrono::Duration::from_std(*duration)
                    .expect("exclude-newer duration is too large");
                Utc::now() - duration
            }
        }
    }
}

impl ResolvedExcludeNewer {
    /// Creates a new configuration from an absolute cutoff date.
    pub fn from_datetime(cutoff: DateTime<Utc>) -> Self {
        Self {
            cutoff,
            channel_cutoffs: BTreeMap::new(),
            package_cutoffs: BTreeMap::new(),
            exemptions: Vec::new(),
            // TODO: After https://github.com/conda/ceps/pull/154 we might need to rethink this
            // https://github.com/prefix-dev/pixi/pull/5848/changes#r3051252281
            include_unknown_timestamp: true,
        }
    }

    /// Adds a channel-specific cutoff override.
    pub fn with_channel_cutoff(mut self, channel: ChannelUrl, cutoff: DateTime<Utc>) -> Self {
        self.channel_cutoffs.insert(channel, cutoff);
        self
    }

    /// Adds a package-specific cutoff override.
    ///
    /// Deprecated in favor of [`Self::with_exemption`], which allows specific
    /// vetted releases instead of lowering the cutoff for every release of a
    /// package.
    #[deprecated(note = "use `with_exemption` to allow specific vetted releases instead")]
    pub fn with_package_cutoff(mut self, package: PackageName, cutoff: DateTime<Utc>) -> Self {
        self.package_cutoffs.insert(package, cutoff);
        self
    }

    /// Exempts records matching `spec` from the cutoff.
    ///
    /// The spec must name exactly one package and must not carry extras, a
    /// condition, or a namespace, as those do not select records. The same
    /// validation the solver applies is run here, so converting the resolved
    /// configuration into a [`rattler_solve::ExcludeNewer`] cannot fail.
    pub fn with_exemption(mut self, spec: MatchSpec) -> Result<Self, InvalidExemptionError> {
        // Validate the way the solver does so the conversion below is
        // infallible.
        rattler_solve::ExcludeNewer::from_datetime(jiff::Timestamp::MAX)
            .with_exemption(spec.clone())?;
        if !self.exemptions.contains(&spec) {
            self.exemptions.push(spec);
        }
        Ok(self)
    }
}

/// Converts a chrono [`DateTime<Utc>`] into the `jiff::Timestamp` that both the
/// conda and the PyPI solver expect for exclude-newer cutoffs.
///
/// chrono spans hundreds of millennia while jiff only spans years -9999..=9999,
/// so out-of-range values saturate rather than fail. This is load bearing:
/// `rattler_solve::ExcludeNewer` requires a non-optional default cutoff, so a
/// workspace with only channel- or package-level cutoffs gets
/// `DateTime::<Utc>::MAX_UTC` as its "never exclude by default" sentinel (see
/// `exclude_newer_config_resolved_impl` in
/// `crates/pixi_manifest/src/features_ext.rs`). Saturating preserves that
/// meaning, since no package is newer than `Timestamp::MAX`.
///
/// Leap seconds are clamped instead of saturated: chrono reports them as a full
/// extra second of subsecond nanos, which jiff rejects, and saturating a cutoff
/// the user actually wrote down would silently stop excluding anything.
pub fn to_saturating_jiff_timestamp(value: DateTime<Utc>) -> jiff::Timestamp {
    let seconds_since_epoch = value.timestamp();
    let nanoseconds = value.timestamp_subsec_nanos().min(999_999_999) as i32;

    jiff::Timestamp::new(seconds_since_epoch, nanoseconds).unwrap_or(if seconds_since_epoch < 0 {
        jiff::Timestamp::MIN
    } else {
        jiff::Timestamp::MAX
    })
}

impl From<ResolvedExcludeNewer> for rattler_solve::ExcludeNewer {
    fn from(value: ResolvedExcludeNewer) -> Self {
        let mut config =
            rattler_solve::ExcludeNewer::from_datetime(to_saturating_jiff_timestamp(value.cutoff))
                .with_timestamp_policy(if value.include_unknown_timestamp {
                    rattler_solve::TimestampPolicy::AllowMissing
                } else {
                    rattler_solve::TimestampPolicy::RequireTimestamp
                });

        for (channel, cutoff) in value.channel_cutoffs {
            config = config
                .with_channel_cutoff(channel.to_string(), to_saturating_jiff_timestamp(cutoff));
        }

        // The deprecated per-package cutoffs keep working until the
        // top-level `[exclude-newer]` manifest table is removed.
        #[allow(deprecated)]
        for (package, cutoff) in value.package_cutoffs {
            config = config.with_package_cutoff(package, to_saturating_jiff_timestamp(cutoff));
        }

        for spec in value.exemptions {
            config = config
                .with_exemption(spec)
                .expect("exemptions are validated when they are added");
        }

        config
    }
}

impl FromStr for ExcludeNewer {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_exclude_newer_str(s)
    }
}

impl std::fmt::Display for ExcludeNewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExcludeNewer::Timestamp(dt) => dt.fmt(f),
            ExcludeNewer::Duration(dur) => humantime::format_duration(*dur).fmt(f),
        }
    }
}

impl serde::Serialize for ExcludeNewer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Timestamp(cutoff) => cutoff.serialize(serializer),
            Self::Duration(duration) => {
                serializer.collect_str(&humantime::Duration::from(*duration))
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for ExcludeNewer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum RawExcludeNewer {
            Timestamp(DateTime<Utc>),
            Duration(String),
        }

        match RawExcludeNewer::deserialize(deserializer)? {
            RawExcludeNewer::Timestamp(cutoff) => Ok(ExcludeNewer::Timestamp(cutoff)),
            RawExcludeNewer::Duration(value) => {
                parse_exclude_newer_str(&value).map_err(serde::de::Error::custom)
            }
        }
    }
}

fn parse_exclude_newer_str(s: &str) -> Result<ExcludeNewer, String> {
    if let Ok(duration) = s.parse::<humantime::Duration>() {
        return Ok(ExcludeNewer::Duration(duration.into()));
    }

    let date_err = match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        Ok(date) => {
            let next_midnight = date
                .checked_add_days(Days::new(1))
                .expect("valid exclude-newer date should have a following day")
                .and_time(NaiveTime::MIN)
                .and_utc();
            return Ok(ExcludeNewer::Timestamp(next_midnight));
        }
        Err(err) => err,
    };

    let timestamp_err = match DateTime::parse_from_rfc3339(s) {
        Ok(timestamp) => return Ok(ExcludeNewer::Timestamp(timestamp.with_timezone(&Utc))),
        Err(err) => err,
    };

    Err(format!(
        "`{s}` is neither a valid duration, date ({date_err}), nor timestamp ({timestamp_err})"
    ))
}

#[cfg(test)]
mod test {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_from_str_timestamp() {
        assert_eq!(
            ExcludeNewer::from_str("2006-12-02T00:00:00Z").unwrap(),
            ExcludeNewer::from_str("2006-12-02T00:00:00+00:00").unwrap(),
        );

        match (
            ExcludeNewer::from_str("2006-12-02T00:00:00Z").unwrap(),
            ExcludeNewer::from_str("2006-12-02T00:00:00+00:00").unwrap(),
        ) {
            (ExcludeNewer::Timestamp(a), ExcludeNewer::Timestamp(b)) => assert_eq!(a, b),
            _ => panic!("expected timestamps"),
        }
    }

    #[test]
    fn test_from_str_date() {
        assert_eq!(
            ExcludeNewer::from_str("2006-12-02").unwrap(),
            ExcludeNewer::from_str("2006-12-03T00:00:00Z").unwrap(),
        );
    }

    #[test]
    fn test_from_str_duration() {
        assert_eq!(
            ExcludeNewer::from_str("7d").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(7 * 24 * 60 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("1h").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(60 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("30m").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(30 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("1h30m").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(90 * 60)),
        );
        assert_eq!(
            ExcludeNewer::from_str("7days").unwrap(),
            ExcludeNewer::Duration(std::time::Duration::from_secs(7 * 24 * 60 * 60)),
        );
    }

    #[test]
    fn test_from_str_invalid_reports_supported_formats() {
        let err = ExcludeNewer::from_str("date").unwrap_err();
        assert!(err.contains("valid duration"), "got: {err}");
        assert!(err.contains("date ("), "got: {err}");
        assert!(err.contains("timestamp ("), "got: {err}");
    }

    #[test]
    fn test_cutoff_for_duration_is_relative_to_now() {
        let before = Utc::now();
        let cutoff = ExcludeNewer::Duration(std::time::Duration::from_secs(60 * 60)).cutoff();
        let after = Utc::now();

        assert!(
            cutoff >= before - chrono::Duration::hours(1) - chrono::Duration::seconds(1),
            "cutoff {cutoff} should be close to one hour before {before}",
        );
        assert!(
            cutoff <= after - chrono::Duration::hours(1) + chrono::Duration::seconds(1),
            "cutoff {cutoff} should be close to one hour before {after}",
        );
    }

    #[test]
    fn test_serde_deserializes_timestamp_with_space_separator() {
        let parsed: ExcludeNewer = serde_json::from_value(json!("2006-12-02 02:07:43Z")).unwrap();

        assert_eq!(
            parsed,
            ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap()
        );
    }

    #[test]
    fn test_serde_roundtrips_duration() {
        let value = ExcludeNewer::Duration(std::time::Duration::from_secs(90 * 60));

        let serialized = serde_json::to_value(value).unwrap();
        assert_eq!(serialized, json!("1h 30m"));

        let deserialized: ExcludeNewer = serde_json::from_value(serialized).unwrap();
        assert_eq!(deserialized, value);
    }

    #[test]
    fn test_display_duration() {
        let d = ExcludeNewer::Duration(std::time::Duration::from_secs(7 * 24 * 60 * 60));
        let display = format!("{d}");
        assert_eq!(display, "7days");
    }

    #[test]
    fn test_display_timestamp() {
        let t = ExcludeNewer::from_str("2006-12-02T02:07:43Z").unwrap();
        let display = format!("{t}");
        assert!(display.contains("2006"), "got: {display}");
    }

    #[test]
    fn test_resolved_into_rattler_solve_preserves_overrides() {
        let default_cutoff = DateTime::parse_from_rfc3339("2006-12-02T02:07:43Z")
            .unwrap()
            .with_timezone(&Utc);
        let channel_cutoff = DateTime::parse_from_rfc3339("2006-12-03T02:07:43Z")
            .unwrap()
            .with_timezone(&Utc);
        let package_cutoff = DateTime::parse_from_rfc3339("2006-12-04T02:07:43Z")
            .unwrap()
            .with_timezone(&Utc);

        #[allow(deprecated)]
        let config: rattler_solve::ExcludeNewer =
            ResolvedExcludeNewer::from_datetime(default_cutoff)
                .with_channel_cutoff(
                    ChannelUrl::from(url::Url::parse("https://prefix.dev/conda-forge").unwrap()),
                    channel_cutoff,
                )
                .with_package_cutoff(PackageName::new_unchecked("foo"), package_cutoff)
                .into();

        assert_eq!(
            config.cutoff_for_package(&PackageName::new_unchecked("baz"), None),
            to_saturating_jiff_timestamp(default_cutoff)
        );
        assert_eq!(
            config.cutoff_for_package(
                &PackageName::new_unchecked("bar"),
                Some("https://prefix.dev/conda-forge/"),
            ),
            to_saturating_jiff_timestamp(channel_cutoff)
        );
        assert_eq!(
            config.cutoff_for_package(
                &PackageName::new_unchecked("foo"),
                Some("https://prefix.dev/conda-forge/"),
            ),
            to_saturating_jiff_timestamp(package_cutoff)
        );
        assert_eq!(
            config.timestamp_policy(),
            rattler_solve::TimestampPolicy::AllowMissing
        );
    }

    #[test]
    fn test_resolved_into_rattler_solve_preserves_exemptions() {
        use rattler_conda_types::{
            PackageRecord, ParseStrictness, RepoDataRecord, Version, package::DistArchiveIdentifier,
        };

        let default_cutoff = DateTime::parse_from_rfc3339("2006-12-02T02:07:43Z")
            .unwrap()
            .with_timezone(&Utc);

        let config: rattler_solve::ExcludeNewer =
            ResolvedExcludeNewer::from_datetime(default_cutoff)
                .with_exemption(
                    MatchSpec::from_str("foo ==1.2.3", ParseStrictness::Strict).unwrap(),
                )
                .unwrap()
                .into();

        let record = |name: &str, version: &str| {
            let mut package_record = PackageRecord::new(
                PackageName::new_unchecked(name),
                Version::from_str(version).unwrap(),
                "0".to_string(),
            );
            package_record.timestamp = Some(
                "2020-01-01T00:00:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .into(),
            );
            RepoDataRecord {
                package_record,
                identifier: DistArchiveIdentifier::from_str(&format!("{name}-{version}-0.conda"))
                    .unwrap(),
                url: url::Url::parse("https://example.com/pkg.conda").unwrap(),
                channel: None,
            }
        };

        assert!(!config.is_excluded(&record("foo", "1.2.3")));
        assert!(config.is_excluded(&record("foo", "1.2.4")));
        assert!(config.is_excluded(&record("bar", "1.2.3")));
    }

    #[test]
    fn test_with_exemption_rejects_invalid_specs() {
        let err = ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC)
            .with_exemption(MatchSpec::default())
            .unwrap_err();
        assert!(matches!(err, InvalidExemptionError::NotExactName(_)));

        let err = ResolvedExcludeNewer::from_datetime(DateTime::<Utc>::MAX_UTC)
            .with_exemption(MatchSpec {
                name: PackageName::new_unchecked("foo").into(),
                extras: Some(vec!["bar".to_string()]),
                ..MatchSpec::default()
            })
            .unwrap_err();
        assert!(matches!(err, InvalidExemptionError::UnsupportedField(_)));
    }

    #[test]
    fn test_out_of_jiff_range_cutoffs_saturate() {
        // `DateTime::<Utc>::MAX_UTC` is the sentinel for "no workspace-wide
        // cutoff", and it lands well past jiff's year 9999 ceiling.
        assert_eq!(
            to_saturating_jiff_timestamp(DateTime::<Utc>::MAX_UTC),
            jiff::Timestamp::MAX
        );
        assert_eq!(
            to_saturating_jiff_timestamp(DateTime::<Utc>::MIN_UTC),
            jiff::Timestamp::MIN
        );
    }
}
