use chrono::{DateTime, Utc};
pub use pixi_exclude_newer::ExcludeNewer;
use rattler_conda_types::{ChannelUrl, MatchSpec, PackageName};
pub use rattler_solve::InvalidExemptionError;
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
};

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

#[cfg(test)]
mod test {
    use super::*;

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
