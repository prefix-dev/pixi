use rattler_build_recipe::stage0::SerializableMatchSpec;
use rattler_conda_types::{MatchSpec, PackageName, ParseMatchSpecError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt::Display, str::FromStr};
use url::Url;

/// A source dependency that pairs a match spec with a source location URL.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SourceMatchSpec {
    /// The match spec for the dependency
    pub spec: MatchSpec,
    /// The URL location of the source
    pub location: Url,
}

/// A dependency that can be either a binary package or a source package.
///
/// Binary dependencies are resolved from channels. Source dependencies reference
/// a source location (URL) and are built from source.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum PackageDependency {
    /// A binary package dependency resolved from channels
    Binary(MatchSpec),
    /// A source dependency with a location URL
    Source(SourceMatchSpec),
}

impl PackageDependency {
    /// Returns the package name for this dependency
    pub fn package_name(&self) -> Option<&PackageName> {
        match self {
            PackageDependency::Binary(spec) => spec.name.as_exact(),
            PackageDependency::Source(source_spec) => source_spec.spec.name.as_exact(),
        }
    }

    /// Returns the inner `MatchSpec` regardless of variant
    pub fn as_match_spec(&self) -> &MatchSpec {
        match self {
            PackageDependency::Binary(spec) => spec,
            PackageDependency::Source(source_spec) => &source_spec.spec,
        }
    }

    /// Returns the source match spec if this is a source dependency
    pub fn as_source(&self) -> Option<&SourceMatchSpec> {
        if let PackageDependency::Source(source_spec) = self {
            Some(source_spec)
        } else {
            None
        }
    }

    /// Check if the dependency can be used as a variant in a recipe
    pub fn can_be_used_as_variant(&self) -> bool {
        match self {
            PackageDependency::Binary(spec) => {
                spec.version == Some(rattler_conda_types::VersionSpec::Any)
                    && spec.build.is_none()
                    && spec.build_number.is_none()
                    && spec.file_name.is_none()
                    && spec.channel.is_none()
                    && spec.subdir.is_none()
                    && spec.md5.is_none()
                    && spec.sha256.is_none()
            }
            _ => false,
        }
    }
}

impl Display for PackageDependency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageDependency::Binary(spec) => write!(f, "{spec}"),
            PackageDependency::Source(source_spec) => {
                // Serialize as matchspec with url
                let mut spec = source_spec.spec.clone();
                spec.url = Some(source_spec.location.clone());
                write!(f, "{spec}")
            }
        }
    }
}

impl FromStr for PackageDependency {
    type Err = ParseMatchSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let spec = MatchSpec::from_str(s, rattler_conda_types::ParseStrictness::Strict)?;
        Ok(PackageDependency::from(SerializableMatchSpec(spec)))
    }
}

impl From<&str> for PackageDependency {
    fn from(s: &str) -> Self {
        s.parse().expect("invalid matchspec")
    }
}

impl From<MatchSpec> for PackageDependency {
    fn from(spec: MatchSpec) -> Self {
        PackageDependency::from(SerializableMatchSpec(spec))
    }
}

impl From<SerializableMatchSpec> for PackageDependency {
    fn from(spec: SerializableMatchSpec) -> Self {
        if let Some(url) = spec.0.url.clone() {
            let mut inner = spec.0;
            let location = url;
            inner.url = None;
            PackageDependency::Source(SourceMatchSpec {
                spec: inner,
                location,
            })
        } else {
            PackageDependency::Binary(spec.0)
        }
    }
}

impl From<PackageDependency> for SerializableMatchSpec {
    fn from(val: PackageDependency) -> Self {
        match val {
            PackageDependency::Binary(spec) => SerializableMatchSpec(spec),
            PackageDependency::Source(source_spec) => {
                let mut matchspec = source_spec.spec;
                matchspec.url = Some(source_spec.location);
                SerializableMatchSpec(matchspec)
            }
        }
    }
}

impl Serialize for PackageDependency {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let ser_matchspec: SerializableMatchSpec = self.clone().into();
        serializer.serialize_str(&ser_matchspec.to_string())
    }
}

impl<'de> Deserialize<'de> for PackageDependency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let spec = MatchSpec::from_str(&s, rattler_conda_types::ParseStrictness::Strict)
            .map(SerializableMatchSpec)
            .map_err(serde::de::Error::custom)?;
        Ok(PackageDependency::from(spec))
    }
}

impl Default for PackageDependency {
    fn default() -> Self {
        PackageDependency::Binary(MatchSpec::default())
    }
}
