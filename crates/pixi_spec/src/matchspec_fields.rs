//! Match-spec selector fields that can decorate a source-side spec
//! (URL source archive, path source, or git checkout).
//!
//! These mirror the subset of `NamelessMatchSpec` that makes sense for
//! source packages once the source has been resolved into a concrete
//! built output: `version`, `build`, `build-number`, `extras`, `flags`,
//! `subdir`, `license`, `license-family`, `when` (parsed into
//! `condition`), and `track-features`. Binary-only fields like
//! `file-name`, `channel`, `md5`, `sha256` deliberately stay on
//! `DetailedSpec` / `UrlBinarySpec` / `PathBinarySpec`.

use rattler_conda_types::{
    BuildNumberSpec, MatchSpecCondition, NamelessMatchSpec, StringMatcher, VersionSpec,
};
use serde_with::{serde_as, skip_serializing_none};

/// Optional match-spec selectors carried alongside a source location.
#[serde_as]
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Hash, PartialEq, Eq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MatchspecFields {
    /// The version spec of the package (e.g. `1.2.3`, `>=1.2.3`, `1.2.*`).
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub version: Option<VersionSpec>,

    /// The build string of the package (e.g. `py37_0`, `py*`).
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build: Option<StringMatcher>,

    /// The build number of the package.
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub build_number: Option<BuildNumberSpec>,

    /// Optional extra dependencies to select for the package.
    pub extras: Option<Vec<String>>,

    /// Plain string flags used to select package variants.
    #[serde_as(as = "Option<Vec<serde_with::DisplayFromStr>>")]
    pub flags: Option<Vec<StringMatcher>>,

    /// The subdir of the channel.
    pub subdir: Option<String>,

    /// The license of the package.
    pub license: Option<String>,

    /// The license family of the package.
    pub license_family: Option<String>,

    /// The condition (`when` in TOML) under which this spec applies.
    pub condition: Option<MatchSpecCondition>,

    /// The track features of the package.
    pub track_features: Option<Vec<String>>,
}

impl MatchspecFields {
    /// Returns `true` if every field is `None`.
    pub fn is_empty(&self) -> bool {
        self.version.is_none()
            && self.build.is_none()
            && self.build_number.is_none()
            && self.extras.is_none()
            && self.flags.is_none()
            && self.subdir.is_none()
            && self.license.is_none()
            && self.license_family.is_none()
            && self.condition.is_none()
            && self.track_features.is_none()
    }

    /// Extract the matchspec subset of a [`NamelessMatchSpec`], ignoring
    /// binary-only fields (`file_name`, `channel`, `md5`, `sha256`, `url`,
    /// `namespace`).
    pub fn from_nameless_match_spec(spec: &NamelessMatchSpec) -> Self {
        Self {
            version: spec.version.clone(),
            build: spec.build.clone(),
            build_number: spec.build_number.clone(),
            extras: spec.extras.clone(),
            flags: spec.flags.clone(),
            subdir: spec.subdir.clone(),
            license: spec.license.clone(),
            license_family: spec.license_family.clone(),
            condition: spec.condition.clone(),
            track_features: spec.track_features.clone(),
        }
    }

    /// Stamp these fields into a [`NamelessMatchSpec`], leaving the
    /// binary-only fields untouched.
    pub fn write_into_nameless_match_spec(&self, spec: &mut NamelessMatchSpec) {
        spec.version = self.version.clone();
        spec.build = self.build.clone();
        spec.build_number = self.build_number.clone();
        spec.extras = self.extras.clone();
        spec.flags = self.flags.clone();
        spec.subdir = self.subdir.clone();
        spec.license = self.license.clone();
        spec.license_family = self.license_family.clone();
        spec.condition = self.condition.clone();
        spec.track_features = self.track_features.clone();
    }

    /// Build a [`NamelessMatchSpec`] populated from these fields only.
    pub fn to_nameless_match_spec(&self) -> NamelessMatchSpec {
        let mut spec = NamelessMatchSpec::default();
        self.write_into_nameless_match_spec(&mut spec);
        spec
    }
}
