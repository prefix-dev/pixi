use chrono::{DateTime, Days, NaiveDate, NaiveTime, Utc};
use rattler_conda_types::{ChannelUrl, MatchSpec, PackageName};
pub use rattler_solve::InvalidExemptionError;
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    str::FromStr,
};

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
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

    /// Records matching any of these exemptions are never excluded, unless
    /// they are newer than the cutoff of the exemption. This allows a single
    /// vetted release through without lowering the cutoff for every future
    /// release of the package.
    ///
    /// Private so that every spec goes through [`validate_exemption`], which
    /// keeps the conversion into a [`rattler_solve::ExcludeNewer`] infallible.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    exemptions: Vec<ResolvedExemption>,

    /// Whether to include packages that don't have a timestamp.
    pub include_unknown_timestamp: bool,

    /// The cutoffs as configured, before relative cutoffs were resolved
    /// against the current time. See [`Self::hash_configured`].
    #[serde(skip)]
    configured: Option<ConfiguredCutoffs>,
}

/// The cutoffs of a [`ResolvedExcludeNewer`] as configured, with relative
/// cutoffs kept as durations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ConfiguredCutoffs {
    /// The configured default cutoff, if any.
    pub cutoff: Option<ExcludeNewer>,

    /// The configured channel-specific cutoffs.
    pub channel_cutoffs: BTreeMap<ChannelUrl, ExcludeNewer>,

    /// The configured package-specific cutoffs of the deprecated top-level
    /// `[exclude-newer]` table.
    pub package_cutoffs: BTreeMap<PackageName, ExcludeNewer>,
}

/// Records matching [`Self::spec`] are exempt from the cutoff of a
/// [`ResolvedExcludeNewer`], up to [`Self::cutoff`] if it is set.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
pub struct ResolvedExemption {
    /// The spec the exempt records match.
    #[serde(serialize_with = "serialize_display")]
    pub spec: MatchSpec,

    /// Records published after this cutoff are not exempt, even if they
    /// match [`Self::spec`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutoff: Option<DateTime<Utc>>,
}

fn serialize_display<S: serde::Serializer>(
    value: &impl std::fmt::Display,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_str(value)
}

impl ExcludeNewer {
    /// Returns the effective cutoff for the current time.
    pub fn cutoff(&self) -> DateTime<Utc> {
        self.cutoff_at(Utc::now())
    }

    /// Returns the effective cutoff if the current time were `now`.
    pub fn cutoff_at(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::Timestamp(cutoff) => *cutoff,
            Self::Duration(duration) => {
                let duration = chrono::Duration::from_std(*duration)
                    .expect("exclude-newer duration is too large");
                now - duration
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
            configured: None,
        }
    }

    /// Records the cutoffs this configuration was resolved from, which
    /// [`Self::hash_configured`] hashes instead of the resolved cutoffs.
    pub fn with_configured_cutoffs(mut self, configured: ConfiguredCutoffs) -> Self {
        self.configured = Some(configured);
        self
    }

    /// Hashes the configuration as configured instead of as resolved, for
    /// cache keys that must be stable across invocations.
    ///
    /// A relative cutoff such as `14d` resolves to a different time on every
    /// invocation, so a key that hashes the resolved cutoffs would never hit
    /// the cache. Falls back to hashing the resolved cutoffs when the
    /// configured cutoffs are unknown.
    pub fn hash_configured<H: Hasher>(&self, state: &mut H) {
        match &self.configured {
            Some(configured) => {
                configured.hash(state);
                self.exemptions.hash(state);
                self.include_unknown_timestamp.hash(state);
            }
            None => self.hash(state),
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
    pub fn with_exemption(self, spec: MatchSpec) -> Result<Self, InvalidExemptionError> {
        self.with_exemption_until(spec, None)
    }

    /// Exempts records matching `spec` that were published up to `cutoff`
    /// from the cutoff, or all of them if `cutoff` is `None`.
    ///
    /// The spec is validated like for [`Self::with_exemption`]. Adding an
    /// exemption for a spec that is already exempt replaces its cutoff.
    pub fn with_exemption_until(
        mut self,
        spec: MatchSpec,
        cutoff: Option<DateTime<Utc>>,
    ) -> Result<Self, InvalidExemptionError> {
        validate_exemption(&spec)?;
        match self
            .exemptions
            .iter_mut()
            .find(|exemption| exemption.spec == spec)
        {
            Some(exemption) => exemption.cutoff = cutoff,
            None => self.exemptions.push(ResolvedExemption { spec, cutoff }),
        }
        Ok(self)
    }

    /// Returns the exemptions from the cutoff.
    pub fn exemptions(&self) -> &[ResolvedExemption] {
        &self.exemptions
    }

    /// Returns the cutoff that applies to `package` from `channel`, the same
    /// way the solver picks it: the package cutoff, then the channel cutoff,
    /// then the default cutoff.
    pub fn cutoff_for(&self, package: &PackageName, channel: &ChannelUrl) -> DateTime<Utc> {
        self.package_cutoffs
            .get(package)
            .or_else(|| self.channel_cutoffs.get(channel))
            .copied()
            .unwrap_or(self.cutoff)
    }

    /// Returns the earliest cutoff that applies to `package` from any channel,
    /// including the channels without a cutoff of their own.
    pub fn earliest_cutoff_for(&self, package: &PackageName) -> DateTime<Utc> {
        match self.package_cutoffs.get(package) {
            Some(cutoff) => *cutoff,
            None => self
                .channel_cutoffs
                .values()
                .copied()
                .fold(self.cutoff, DateTime::min),
        }
    }
}

/// Validates an exemption the way the solver does, so that converting a
/// [`ResolvedExcludeNewer`] into a [`rattler_solve::ExcludeNewer`] cannot fail.
fn validate_exemption(spec: &MatchSpec) -> Result<(), InvalidExemptionError> {
    rattler_solve::ExcludeNewer::from_datetime(jiff::Timestamp::MAX)
        .with_exemption(spec.clone())
        .map(|_| ())
}

/// Converts a chrono [`DateTime<Utc>`] into the `jiff::Timestamp` that both the
/// conda and the PyPI solver expect for exclude-newer cutoffs.
///
/// chrono spans hundreds of millennia while jiff only spans years -9999..=9999,
/// so out-of-range values saturate rather than fail. This is load bearing:
/// `rattler_solve::ExcludeNewer` requires a non-optional default cutoff, so a
/// workspace with only channel- or package-level cutoffs gets
/// `DateTime::<Utc>::MAX_UTC` as its "never exclude by default" sentinel (see
/// `resolve_exclude_newer` in `crates/pixi_manifest/src/exclude_newer.rs`).
/// Saturating preserves that meaning, since no package is newer than
/// `Timestamp::MAX`.
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

        for ResolvedExemption { spec, cutoff } in value.exemptions {
            config = match cutoff {
                Some(cutoff) => {
                    config.with_exemption_until(spec, to_saturating_jiff_timestamp(cutoff))
                }
                None => config.with_exemption(spec),
            }
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

// TODO: `rattler_config` parses the same cutoffs with its own, private parser.
// Once it exposes it, use it here so the two can't drift apart.
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
        use pixi_test_utils::uploaded_record;
        use rattler_conda_types::ParseStrictness;

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

        assert!(!config.is_excluded(&uploaded_record("foo", "1.2.3", None)));
        assert!(config.is_excluded(&uploaded_record("foo", "1.2.4", None)));
        assert!(config.is_excluded(&uploaded_record("bar", "1.2.3", None)));
    }

    #[test]
    fn test_resolved_into_rattler_solve_preserves_exemption_cutoffs() {
        use pixi_test_utils::uploaded_record;
        use rattler_conda_types::ParseStrictness;

        // `uploaded_record` creates records uploaded at 2020-01-01T00:00:00Z.
        let config = |exemption_cutoff: &str| -> rattler_solve::ExcludeNewer {
            ResolvedExcludeNewer::from_datetime("2019-01-01T00:00:00Z".parse().unwrap())
                .with_exemption_until(
                    MatchSpec::from_str("foo ==1.2.3", ParseStrictness::Strict).unwrap(),
                    Some(exemption_cutoff.parse().unwrap()),
                )
                .unwrap()
                .into()
        };

        let record = uploaded_record("foo", "1.2.3", None);
        assert!(!config("2020-01-01T00:00:00Z").is_excluded(&record));
        assert!(config("2019-12-31T23:59:59Z").is_excluded(&record));
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
