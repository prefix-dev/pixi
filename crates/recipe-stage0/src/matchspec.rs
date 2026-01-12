use rattler_conda_types::{MatchSpec, PackageName, ParseMatchSpecError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt::Display, str::FromStr};
use url::Url;

// Wrapper for MatchSpec to enable serde support
#[derive(Debug, Clone, Default)]
pub struct SerializableMatchSpec(pub MatchSpec);

impl Serialize for SerializableMatchSpec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for SerializableMatchSpec {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        MatchSpec::from_str(&s, rattler_conda_types::ParseStrictness::Strict)
            .map(SerializableMatchSpec)
            .map_err(serde::de::Error::custom)
    }
}

impl From<MatchSpec> for SerializableMatchSpec {
    fn from(spec: MatchSpec) -> Self {
        SerializableMatchSpec(spec)
    }
}

impl From<&str> for SerializableMatchSpec {
    fn from(s: &str) -> Self {
        SerializableMatchSpec(
            MatchSpec::from_str(s, rattler_conda_types::ParseStrictness::Strict)
                .expect("Invalid MatchSpec"),
        )
    }
}

impl From<String> for SerializableMatchSpec {
    fn from(s: String) -> Self {
        SerializableMatchSpec(
            MatchSpec::from_str(&s, rattler_conda_types::ParseStrictness::Strict)
                .expect("Invalid MatchSpec"),
        )
    }
}

impl Display for SerializableMatchSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for SerializableMatchSpec {
    type Err = ParseMatchSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        MatchSpec::from_str(s, rattler_conda_types::ParseStrictness::Strict)
            .map(SerializableMatchSpec)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct SourceMatchSpec {
    pub spec: MatchSpec,
    pub location: Url,
}

#[derive(Clone, PartialEq, Debug)]
pub enum PackageDependency {
    Binary(MatchSpec),
    Source(SourceMatchSpec),
}

impl PackageDependency {
    pub fn package_name(&self) -> PackageName {
        match self {
            PackageDependency::Binary(spec) => spec
                .name
                .as_ref()
                .and_then(|matcher| matcher.as_exact())
                .cloned()
                .expect("Binary spec should have a name"),
            PackageDependency::Source(source_spec) => source_spec
                .spec
                .name
                .as_ref()
                .and_then(|matcher| matcher.as_exact())
                .cloned()
                .expect("Source spec should have a name"),
        }
    }

    pub fn as_source(&self) -> Option<&SourceMatchSpec> {
        if let PackageDependency::Source(source_spec) = self {
            Some(source_spec)
        } else {
            None
        }
    }

    /// Check if the dependency can be used as a variant in a recipe.
    pub fn can_be_used_as_variant(&self) -> bool {
        match self {
            PackageDependency::Binary(boxed_spec) => {
                let rattler_conda_types::MatchSpec {
                    version,
                    build,
                    build_number,
                    file_name,
                    channel,
                    subdir,
                    md5,
                    sha256,
                    ..
                } = boxed_spec;

                version == &Some(rattler_conda_types::VersionSpec::Any)
                    && build.is_none()
                    && build_number.is_none()
                    && file_name.is_none()
                    && channel.is_none()
                    && subdir.is_none()
                    && md5.is_none()
                    && sha256.is_none()
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
                let matchspec = SerializableMatchSpec::from(source_spec.spec.clone());
                write!(f, "Source(spec: {}, {})", matchspec, source_spec.location)
            }
        }
    }
}

impl From<SerializableMatchSpec> for PackageDependency {
    fn from(spec: SerializableMatchSpec) -> Self {
        // we need to split on url to determine if this is a binary or source dependency
        if let Some(url) = spec.0.url.as_ref() {
            // remove the URL from the MatchSpec
            let mut spec = spec.0.clone();
            spec.url = None;

            PackageDependency::Source(SourceMatchSpec {
                spec,
                location: url.clone(),
            })
        } else {
            PackageDependency::Binary(spec.0)
        }
    }
}

impl FromStr for PackageDependency {
    type Err = ParseMatchSpecError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        SerializableMatchSpec::from_str(s).map(PackageDependency::from)
    }
}

impl From<&str> for PackageDependency {
    fn from(s: &str) -> Self {
        SerializableMatchSpec::from(s).into()
    }
}

impl From<PackageDependency> for SerializableMatchSpec {
    fn from(val: PackageDependency) -> Self {
        match val {
            PackageDependency::Binary(spec) => SerializableMatchSpec(spec),
            PackageDependency::Source(source_spec) => {
                // we need to put the URL from source spec back into the MatchSpec

                let mut matchspec = source_spec.spec.clone();
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
        let dep = self.clone();

        let ser_matchspec: SerializableMatchSpec = dep.into();
        serializer.serialize_str(&ser_matchspec.to_string())
    }
}

impl<'de> Deserialize<'de> for PackageDependency {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        MatchSpec::from_str(&s, rattler_conda_types::ParseStrictness::Strict)
            .map(SerializableMatchSpec)
            .map(PackageDependency::from)
            .map_err(serde::de::Error::custom)
    }
}
