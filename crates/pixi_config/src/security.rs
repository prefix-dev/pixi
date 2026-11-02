//! The `[security]` configuration table: whether untrusted code may run, and
//! the list of packages that may run code while it may not.
//!
//! This module only holds the data model and its validation. Turning the
//! switch on from the command line and enforcing the list happen elsewhere.

use std::str::FromStr;

use pixi_exclude_newer::ExcludeNewer;
use rattler_conda_types::{ChannelUrl, PackageName, ParseStrictness, VersionSpec};
use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

/// The `[security]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SecurityConfig {
    /// Whether code pixi did not explicitly trust may run. Defaults to
    /// `true`.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub untrusted_code_exec: Option<bool>,

    /// The packages that may run code while untrusted code execution is
    /// disabled. `None` and `Some(vec![])` both trust nothing; they are kept
    /// apart so that a layer setting an empty list replaces the list of the
    /// layers below it.
    #[serde(default, deserialize_with = "deserialize_some")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_packages: Option<Vec<TrustedPackagesRule>>,
}

/// Deserializes a present value as `Some` without going through serde's
/// `Option` handling, which would add a `?` segment to the path of an
/// unknown-key warning (`security.trusted-packages.?.0.reject-neweer`).
fn deserialize_some<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl SecurityConfig {
    /// Whether no key of the table is set.
    pub fn is_default(&self) -> bool {
        self.untrusted_code_exec.is_none() && self.trusted_packages.is_none()
    }

    /// Merges a higher-priority layer into this one. Every key the other
    /// layer sets replaces the value of this layer; the trusted-packages
    /// lists are not concatenated.
    pub fn merge(self, other: Self) -> Self {
        Self {
            untrusted_code_exec: other.untrusted_code_exec.or(self.untrusted_code_exec),
            trusted_packages: other.trusted_packages.or(self.trusted_packages),
        }
    }

    /// Whether untrusted code may run (defaults to `true`).
    pub fn untrusted_code_exec(&self) -> bool {
        self.untrusted_code_exec.unwrap_or(true)
    }

    /// The trust rules; a missing list trusts nothing.
    pub fn trusted_packages(&self) -> &[TrustedPackagesRule] {
        self.trusted_packages.as_deref().unwrap_or_default()
    }
}

/// One entry of `[[security.trusted-packages]]`: the packages from one
/// channel that may run code while untrusted code execution is disabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TrustedPackagesRule {
    /// The channel the packages must come from, as a full URL.
    pub channel: TrustedChannelUrl,

    /// Packages uploaded after this cutoff are not trusted by this rule.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reject_newer: Option<ExcludeNewer>,

    /// The trusted packages. May be empty.
    pub packages: Vec<TrustedPackage>,
}

/// A package of a [`TrustedPackagesRule`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedPackage {
    /// The normalized conda package name.
    pub name: PackageName,

    /// The versions that are trusted. `"*"` trusts every version.
    #[serde(deserialize_with = "deserialize_strict_version_spec")]
    pub version: VersionSpec,
}

/// Parses a version constraint the way manifests do: strictly, so that a
/// constraint in a security list means exactly what it says.
fn deserialize_strict_version_spec<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<VersionSpec, D::Error> {
    let spec = String::deserialize(deserializer)?;
    VersionSpec::from_str(&spec, ParseStrictness::Strict).map_err(serde::de::Error::custom)
}

/// A channel URL of a trust rule.
///
/// Compared like a [`ChannelUrl`]: by scheme, host, port and path, with the
/// trailing slash normalized. Unlike a plain [`ChannelUrl`], parsing rejects
/// channel names and URLs with userinfo, a query or a fragment, since those
/// can be made to look like a trusted channel while pointing elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash, derive_more::Display)]
pub struct TrustedChannelUrl(ChannelUrl);

impl TrustedChannelUrl {
    /// The channel URL this rule applies to.
    pub fn channel_url(&self) -> &ChannelUrl {
        &self.0
    }
}

/// Why a string is not a valid [`TrustedChannelUrl`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TrustedChannelUrlError {
    #[error(
        "`{value}` is not a URL: the channel of a trust rule must be a full URL like `https://prefix.dev/conda-forge`, not a channel name"
    )]
    NotAUrl {
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error("`{0}` contains a username or password; use `pixi auth login` for credentials")]
    UserInfo(String),

    #[error("`{0}` contains a query string, which a channel URL must not have")]
    Query(String),

    #[error("`{0}` contains a fragment, which a channel URL must not have")]
    Fragment(String),
}

impl FromStr for TrustedChannelUrl {
    type Err = TrustedChannelUrlError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(value).map_err(|source| TrustedChannelUrlError::NotAUrl {
            value: value.to_string(),
            source,
        })?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(TrustedChannelUrlError::UserInfo(value.to_string()));
        }
        if url.query().is_some() {
            return Err(TrustedChannelUrlError::Query(value.to_string()));
        }
        if url.fragment().is_some() {
            return Err(TrustedChannelUrlError::Fragment(value.to_string()));
        }
        Ok(Self(ChannelUrl::from(url)))
    }
}

impl<'de> Deserialize<'de> for TrustedChannelUrl {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl Serialize for TrustedChannelUrl {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(url: &str) -> TrustedChannelUrl {
        TrustedChannelUrl::from_str(url).unwrap()
    }

    #[test]
    fn channel_url_rejects_channel_names() {
        let err = TrustedChannelUrl::from_str("conda-forge").unwrap_err();
        assert!(
            matches!(err, TrustedChannelUrlError::NotAUrl { .. }),
            "{err}"
        );
        assert!(err.to_string().contains("not a channel name"), "{err}");
    }

    #[test]
    fn channel_url_rejects_userinfo_query_and_fragment() {
        assert!(matches!(
            TrustedChannelUrl::from_str("https://user:pw@prefix.dev/conda-forge").unwrap_err(),
            TrustedChannelUrlError::UserInfo(_)
        ));
        assert!(matches!(
            TrustedChannelUrl::from_str("https://user@prefix.dev/conda-forge").unwrap_err(),
            TrustedChannelUrlError::UserInfo(_)
        ));
        assert!(matches!(
            TrustedChannelUrl::from_str("https://prefix.dev/conda-forge?x=1").unwrap_err(),
            TrustedChannelUrlError::Query(_)
        ));
        assert!(matches!(
            TrustedChannelUrl::from_str("https://prefix.dev/conda-forge?").unwrap_err(),
            TrustedChannelUrlError::Query(_)
        ));
        assert!(matches!(
            TrustedChannelUrl::from_str("https://prefix.dev/conda-forge#x").unwrap_err(),
            TrustedChannelUrlError::Fragment(_)
        ));
    }

    #[test]
    fn channel_url_accepts_every_scheme_of_a_full_url() {
        trusted("https://prefix.dev/conda-forge");
        trusted("http://localhost:8000/channel");
        trusted("s3://my-bucket/channel");
        trusted("file:///tmp/channel");
    }

    #[test]
    fn channel_url_normalizes_trailing_slash_and_default_port() {
        let reference = trusted("https://conda.anaconda.org/conda-forge");
        assert_eq!(
            trusted("https://conda.anaconda.org/conda-forge/"),
            reference
        );
        assert_eq!(
            trusted("https://conda.anaconda.org:443/conda-forge"),
            reference
        );
        assert_eq!(trusted("HTTPS://CONDA.ANACONDA.ORG/conda-forge"), reference);
        assert_eq!(
            reference.to_string(),
            "https://conda.anaconda.org/conda-forge/"
        );
    }

    /// The look-alike URLs a repository could put in its manifest. None of
    /// them may compare equal to the trusted channel; some do not even parse.
    #[test]
    fn channel_url_decoys_never_match() {
        let reference = trusted("https://conda.anaconda.org/conda-forge");
        let decoys = [
            "https://conda.anaconda.org@evil.com/conda-forge",
            "https://conda.anaconda.org.evil.com/conda-forge",
            "https://conda.anaconda.org/t/x/conda-forge",
            "https://conda.anaconda.org/conda-forge/../evil",
            "https://conda.anaconda.org/conda-forge%2F..%2Fevil",
            "https://conda.anaconda.org/conda-forge/label/x",
            "http://conda.anaconda.org/conda-forge",
            "https://conda.anaconda.org/",
            "https://conda.anaconda.org/conda-forge/linux-64",
            "https://conda.anaconda.org:8443/conda-forge",
        ];
        for decoy in decoys {
            if let Ok(parsed) = TrustedChannelUrl::from_str(decoy) {
                assert_ne!(parsed, reference, "`{decoy}` must not match");
            }
        }
    }

    #[test]
    fn channel_url_dot_segments_are_resolved_before_comparing() {
        assert_eq!(
            trusted("https://conda.anaconda.org/conda-forge/../evil"),
            trusted("https://conda.anaconda.org/evil")
        );
    }

    #[test]
    fn channel_url_serializes_without_trailing_slash() {
        let json = serde_json::to_string(&trusted("https://prefix.dev/conda-forge/")).unwrap();
        assert_eq!(json, r#""https://prefix.dev/conda-forge""#);
        let parsed: TrustedChannelUrl = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, trusted("https://prefix.dev/conda-forge"));
    }

    #[test]
    fn rule_parses_every_field() {
        let rule: TrustedPackagesRule = toml::from_str(
            r#"
            channel = "https://prefix.dev/conda-forge"
            reject-newer = "7d"
            packages = [
                { name = "pixi-build-python", version = ">=0.8.6,!=0.8.7" },
                { name = "Pixi-Build-API-Version", version = "*" },
            ]
            "#,
        )
        .unwrap();
        assert_eq!(rule.channel, trusted("https://prefix.dev/conda-forge"));
        assert_eq!(
            rule.reject_newer,
            Some(ExcludeNewer::from_str("7d").unwrap())
        );
        assert_eq!(rule.packages.len(), 2);
        assert_eq!(rule.packages[0].name.as_normalized(), "pixi-build-python");
        assert_eq!(
            rule.packages[0].version,
            VersionSpec::from_str(">=0.8.6,!=0.8.7", ParseStrictness::Strict).unwrap()
        );
        assert_eq!(
            rule.packages[1].name.as_normalized(),
            "pixi-build-api-version"
        );
        assert_eq!(rule.packages[1].version, VersionSpec::Any);
    }

    #[test]
    fn rule_requires_channel_and_packages() {
        let err = toml::from_str::<TrustedPackagesRule>(r#"packages = []"#).unwrap_err();
        assert!(err.to_string().contains("channel"), "{err}");

        let err =
            toml::from_str::<TrustedPackagesRule>(r#"channel = "https://prefix.dev/conda-forge""#)
                .unwrap_err();
        assert!(err.to_string().contains("packages"), "{err}");
    }

    #[test]
    fn package_requires_name_and_version() {
        let err = toml::from_str::<TrustedPackage>(r#"name = "foo""#).unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");

        let err = toml::from_str::<TrustedPackage>(r#"version = "*""#).unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");
    }

    #[test]
    fn package_rejects_invalid_name_and_version() {
        let package = |name: &str, version: &str| {
            toml::from_str::<TrustedPackage>(&format!("name = {name:?}\nversion = {version:?}"))
        };
        assert!(package("foo", "*").is_ok());
        assert!(package("foo", ">=1.2,<2").is_ok());

        let err = package("not a name", "*").unwrap_err();
        assert!(err.to_string().contains("name"), "{err}");

        let err = package("foo", "not a version").unwrap_err();
        assert!(err.to_string().contains("version"), "{err}");

        // Lenient spellings that manifests also reject.
        assert!(package("foo", ">=1.2.*").is_err());
        assert!(package("foo", "1.2").is_err());
    }

    #[test]
    fn rule_rejects_the_manifest_table_form_of_reject_newer() {
        let err = toml::from_str::<TrustedPackagesRule>(
            r#"
            channel = "https://prefix.dev/conda-forge"
            reject-newer = { cutoff = "7d" }
            packages = []
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("reject-newer"), "{err}");
    }

    #[test]
    fn config_merge_replaces_each_key() {
        let lower = SecurityConfig {
            untrusted_code_exec: Some(false),
            trusted_packages: Some(vec![TrustedPackagesRule {
                channel: trusted("https://prefix.dev/conda-forge"),
                reject_newer: None,
                packages: vec![],
            }]),
        };

        // An unset layer keeps everything.
        assert_eq!(lower.clone().merge(SecurityConfig::default()), lower);

        // A set key replaces the value, including an empty list.
        let upper = SecurityConfig {
            untrusted_code_exec: Some(true),
            trusted_packages: Some(vec![]),
        };
        assert_eq!(lower.merge(upper.clone()), upper);
    }

    #[test]
    fn config_defaults_allow_untrusted_code_and_trust_nothing() {
        let config = SecurityConfig::default();
        assert!(config.untrusted_code_exec());
        assert!(config.trusted_packages().is_empty());
        assert!(config.is_default());
    }
}
