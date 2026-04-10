use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use rattler_conda_types::{ChannelUrl, PackageName};
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
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub package_cutoffs: BTreeMap<PackageName, DateTime<Utc>>,

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
    pub fn with_package_cutoff(mut self, package: PackageName, cutoff: DateTime<Utc>) -> Self {
        self.package_cutoffs.insert(package, cutoff);
        self
    }

    /// Constrains this exclude-newer configuration using concrete timestamps
    /// from a previous solve. For each dimension (default, per-channel,
    /// per-package) that already exists in this configuration, the cutoff is
    /// tightened to the minimum of the current value and the source timestamp.
    /// Dimensions not present in `self` are left unchanged — no new entries
    /// are added.
    #[cfg(feature = "rattler_lock")]
    pub fn constraint_to_timestamps(mut self, timestamps: &rattler_lock::SourceTimestamps) -> Self {
        self.cutoff = self.cutoff.min(timestamps.latest);

        for (channel, existing) in &mut self.channel_cutoffs {
            if let Some(Some(ts)) = timestamps.channels.get(channel) {
                *existing = (*existing).min(*ts);
            }
        }

        for (package, existing) in &mut self.package_cutoffs {
            if let Some(Some(ts)) = timestamps.packages.get(package) {
                *existing = (*existing).min(*ts);
            }
        }

        self
    }

    /// Returns `true` if the given source timestamps are satisfied by this
    /// exclude-newer configuration — i.e. every timestamp falls within the
    /// corresponding cutoff.
    ///
    /// The matching rules are:
    /// - A package override in `timestamps` is checked against the matching
    ///   `package_cutoffs` entry, falling back to `cutoff`.
    /// - A channel override in `timestamps` is checked against the matching
    ///   `channel_cutoffs` entry, falling back to `cutoff`.
    /// - An entry in `channel_cutoffs` not present in `timestamps` is checked
    ///   against `timestamps.latest`.
    /// - An entry in `package_cutoffs` not present in `timestamps` is checked
    ///   against `timestamps.latest`.
    /// - `timestamps.latest` is checked against `cutoff`.
    #[cfg(feature = "rattler_lock")]
    pub fn is_satisfied_by(&self, timestamps: &rattler_lock::SourceTimestamps) -> bool {
        // Check default timestamp against default cutoff.
        if timestamps.latest > self.cutoff {
            return false;
        }

        // Check each channel in the timestamps against the matching cutoff
        // (or the default cutoff if no channel-specific cutoff exists).
        for (channel, ts) in &timestamps.channels {
            if let Some(ts) = ts {
                let cutoff = self
                    .channel_cutoffs
                    .get(channel)
                    .copied()
                    .unwrap_or(self.cutoff);
                if *ts > cutoff {
                    return false;
                }
            }
        }

        // Check each package in the timestamps against the matching cutoff
        // (or the default cutoff if no package-specific cutoff exists).
        for (package, ts) in &timestamps.packages {
            if let Some(ts) = ts {
                let cutoff = self
                    .package_cutoffs
                    .get(package)
                    .copied()
                    .unwrap_or(self.cutoff);
                if *ts > cutoff {
                    return false;
                }
            }
        }

        // Check channel cutoffs that are in exclude_newer but not in timestamps
        // — compare against the default timestamp.
        for (channel, cutoff) in &self.channel_cutoffs {
            if !timestamps.channels.contains_key(channel) && timestamps.latest > *cutoff {
                return false;
            }
        }

        // Check package cutoffs that are in exclude_newer but not in timestamps
        // — compare against the default timestamp.
        for (package, cutoff) in &self.package_cutoffs {
            if !timestamps.packages.contains_key(package) && timestamps.latest > *cutoff {
                return false;
            }
        }

        true
    }
}

/// Convert [`crate::SourceTimestamps`] into a [`ResolvedExcludeNewer`].
///
/// [`SourceTimestamps`] records the concrete timestamps of the newest packages
/// in the build/host environments stored in the lock file.
/// [`ResolvedExcludeNewer`] is the solve-time cutoff configuration that
/// constrains which packages are visible. This conversion bridges the two so
/// that a locked source record can be re-solved with the same constraints.
#[cfg(feature = "rattler_lock")]
impl From<crate::SourceTimestamps> for ResolvedExcludeNewer {
    fn from(value: crate::SourceTimestamps) -> Self {
        let mut result = Self::from_datetime(value.latest);

        for (channel, cutoff) in value.channels {
            if let Some(cutoff) = cutoff {
                result = result.with_channel_cutoff(channel, cutoff);
            }
        }

        for (package, cutoff) in value.packages {
            if let Some(cutoff) = cutoff {
                result = result.with_package_cutoff(package, cutoff);
            }
        }

        result
    }
}

impl From<ResolvedExcludeNewer> for rattler_solve::ExcludeNewer {
    fn from(value: ResolvedExcludeNewer) -> Self {
        let mut config = rattler_solve::ExcludeNewer::from_datetime(value.cutoff)
            .with_include_unknown_timestamp(value.include_unknown_timestamp);

        for (channel, cutoff) in value.channel_cutoffs {
            config = config.with_channel_cutoff(channel.to_string(), cutoff);
        }

        for (package, cutoff) in value.package_cutoffs {
            config = config.with_package_cutoff(package, cutoff);
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
            default_cutoff
        );
        assert_eq!(
            config.cutoff_for_package(
                &PackageName::new_unchecked("bar"),
                Some("https://prefix.dev/conda-forge/"),
            ),
            channel_cutoff
        );
        assert_eq!(
            config.cutoff_for_package(
                &PackageName::new_unchecked("foo"),
                Some("https://prefix.dev/conda-forge/"),
            ),
            package_cutoff
        );
        assert!(config.include_unknown_timestamp());
    }

    #[cfg(feature = "rattler_lock")]
    mod is_satisfied_by_tests {
        use super::*;
        use rattler_lock::SourceTimestamps;

        fn ts(s: &str) -> DateTime<Utc> {
            DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
        }

        fn channel(s: &str) -> ChannelUrl {
            ChannelUrl::from(url::Url::parse(s).unwrap())
        }

        #[test]
        fn default_within_cutoff() {
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-06-01T00:00:00Z"));
            let timestamps = SourceTimestamps::from_default(ts("2026-05-01T00:00:00Z"));
            assert!(exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn default_exceeds_cutoff() {
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-01-01T00:00:00Z"));
            let timestamps = SourceTimestamps::from_default(ts("2026-06-01T00:00:00Z"));
            assert!(!exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn channel_timestamp_within_channel_cutoff() {
            let ch = channel("https://conda.anaconda.org/conda-forge");
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-01-01T00:00:00Z"))
                .with_channel_cutoff(ch.clone(), ts("2026-06-01T00:00:00Z"));
            let timestamps = SourceTimestamps::from_default(ts("2026-01-01T00:00:00Z"))
                .with_channel(ch, Some(ts("2026-05-01T00:00:00Z")));
            assert!(exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn channel_timestamp_exceeds_channel_cutoff() {
            let ch = channel("https://conda.anaconda.org/conda-forge");
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-01-01T00:00:00Z"))
                .with_channel_cutoff(ch.clone(), ts("2026-03-01T00:00:00Z"));
            let timestamps = SourceTimestamps::from_default(ts("2026-01-01T00:00:00Z"))
                .with_channel(ch, Some(ts("2026-06-01T00:00:00Z")));
            assert!(!exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn channel_in_timestamps_not_in_exclude_newer_uses_default_cutoff() {
            let ch = channel("https://conda.anaconda.org/conda-forge");
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-03-01T00:00:00Z"));
            // Channel timestamp within default cutoff.
            let timestamps = SourceTimestamps::from_default(ts("2026-01-01T00:00:00Z"))
                .with_channel(ch.clone(), Some(ts("2026-02-01T00:00:00Z")));
            assert!(exclude_newer.is_satisfied_by(&timestamps));
            // Channel timestamp exceeds default cutoff.
            let timestamps = SourceTimestamps::from_default(ts("2026-01-01T00:00:00Z"))
                .with_channel(ch, Some(ts("2026-06-01T00:00:00Z")));
            assert!(!exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn channel_in_exclude_newer_not_in_timestamps_uses_default_timestamp() {
            let ch = channel("https://conda.anaconda.org/conda-forge");
            // Default timestamp within channel cutoff.
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-06-01T00:00:00Z"))
                .with_channel_cutoff(ch.clone(), ts("2026-04-01T00:00:00Z"));
            let timestamps = SourceTimestamps::from_default(ts("2026-03-01T00:00:00Z"));
            assert!(exclude_newer.is_satisfied_by(&timestamps));
            // Default timestamp exceeds channel cutoff.
            let timestamps = SourceTimestamps::from_default(ts("2026-05-01T00:00:00Z"));
            assert!(!exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn package_timestamp_within_package_cutoff() {
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-01-01T00:00:00Z"))
                .with_package_cutoff(
                    PackageName::new_unchecked("numpy"),
                    ts("2026-06-01T00:00:00Z"),
                );
            let timestamps = SourceTimestamps::from_default(ts("2026-01-01T00:00:00Z"))
                .with_package(
                    PackageName::new_unchecked("numpy"),
                    Some(ts("2026-05-01T00:00:00Z")),
                );
            assert!(exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn package_timestamp_exceeds_package_cutoff() {
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-01-01T00:00:00Z"))
                .with_package_cutoff(
                    PackageName::new_unchecked("numpy"),
                    ts("2026-03-01T00:00:00Z"),
                );
            let timestamps = SourceTimestamps::from_default(ts("2026-01-01T00:00:00Z"))
                .with_package(
                    PackageName::new_unchecked("numpy"),
                    Some(ts("2026-06-01T00:00:00Z")),
                );
            assert!(!exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn package_in_exclude_newer_not_in_timestamps_uses_default_timestamp() {
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-06-01T00:00:00Z"))
                .with_package_cutoff(
                    PackageName::new_unchecked("numpy"),
                    ts("2026-04-01T00:00:00Z"),
                );
            // Default timestamp within package cutoff.
            let timestamps = SourceTimestamps::from_default(ts("2026-03-01T00:00:00Z"));
            assert!(exclude_newer.is_satisfied_by(&timestamps));
            // Default timestamp exceeds package cutoff.
            let timestamps = SourceTimestamps::from_default(ts("2026-05-01T00:00:00Z"));
            assert!(!exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn none_timestamps_are_ignored() {
            let ch = channel("https://conda.anaconda.org/conda-forge");
            let exclude_newer = ResolvedExcludeNewer::from_datetime(ts("2026-01-01T00:00:00Z"))
                .with_channel_cutoff(ch.clone(), ts("2026-01-01T00:00:00Z"));
            // None means "not used" — should not cause a failure.
            let timestamps = SourceTimestamps::from_default(ts("2026-01-01T00:00:00Z"))
                .with_channel(ch, None)
                .with_package(PackageName::new_unchecked("numpy"), None);
            assert!(exclude_newer.is_satisfied_by(&timestamps));
        }

        #[test]
        fn exact_match_is_satisfied() {
            let ch = channel("https://conda.anaconda.org/conda-forge");
            let t = ts("2026-04-01T00:00:00Z");
            let exclude_newer =
                ResolvedExcludeNewer::from_datetime(t).with_channel_cutoff(ch.clone(), t);
            let timestamps = SourceTimestamps::from_default(t).with_channel(ch, Some(t));
            assert!(exclude_newer.is_satisfied_by(&timestamps));
        }
    }
}
