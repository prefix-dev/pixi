use std::{fmt::Display, sync::Arc};

use crate::BinarySpec;
use rattler_conda_types::{
    BuildNumberSpec, ChannelConfig, NamedChannelOrUrl, NamelessMatchSpec, ParseChannelError,
    StringMatcher, VersionSpec,
};
use rattler_digest::{Md5Hash, Sha256Hash};
use serde_with::{serde_as, skip_serializing_none};

/// A specification for a package in a conda channel.
///
/// This type maps closely to [`rattler_conda_types::NamelessMatchSpec`] but
/// does not represent a `url` field. To represent a `url` spec, use
/// [`crate::UrlSpec`] instead.
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Clone, Hash, Eq, Default, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct DetailedSpec {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub version: Option<VersionSpec>,

    /// The build string of the package (e.g. `py37_0`, `py37h6de7cb9_0`, `py*`)
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build: Option<StringMatcher>,

    /// The build number of the package
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build_number: Option<BuildNumberSpec>,

    /// Match the specific filename of the package
    pub file_name: Option<String>,

    /// The channel of the package
    pub channel: Option<NamedChannelOrUrl>,

    /// The subdir of the channel
    pub subdir: Option<String>,

    /// The license
    pub license: Option<String>,

    /// The md5 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<Md5Hash>,

    /// The sha256 hash of the package
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<Sha256Hash>,
}

impl DetailedSpec {
    /// Converts this instance into a [`NamelessMatchSpec`].
    pub fn try_into_nameless_match_spec(
        self,
        channel_config: &ChannelConfig,
    ) -> Result<NamelessMatchSpec, ParseChannelError> {
        Ok(NamelessMatchSpec {
            version: self.version,
            build: self.build,
            build_number: self.build_number,
            file_name: self.file_name,
            channel: self
                .channel
                .map(|c| c.into_channel(channel_config))
                .transpose()?
                .map(Arc::new),
            subdir: self.subdir,
            namespace: None,
            md5: self.md5,
            sha256: self.sha256,
            license: self.license,
            url: None,
            extras: Default::default(),
        })
    }
}

impl From<DetailedSpec> for BinarySpec {
    fn from(value: DetailedSpec) -> Self {
        Self::DetailedVersion(Box::new(value))
    }
}

impl Display for DetailedSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();

        if let Some(version) = &self.version {
            parts.push(format!("version={}", version));
        }

        if let Some(build) = &self.build {
            parts.push(format!("build={}", build));
        }

        if let Some(build_number) = &self.build_number {
            parts.push(format!("build_number={}", build_number));
        }

        if let Some(file_name) = &self.file_name {
            parts.push(format!("file_name={}", file_name));
        }

        if let Some(channel) = &self.channel {
            parts.push(format!("channel={}", channel));
        }

        if let Some(subdir) = &self.subdir {
            parts.push(format!("subdir={}", subdir));
        }

        if let Some(license) = &self.license {
            parts.push(format!("license={}", license));
        }

        if let Some(md5) = &self.md5 {
            parts.push(format!("md5={:x}", md5));
        }

        if let Some(sha256) = &self.sha256 {
            parts.push(format!("sha256={:x}", sha256));
        }

        if parts.is_empty() {
            write!(f, "*")
        } else {
            write!(f, "{}", parts.join(" "))
        }
    }
}
