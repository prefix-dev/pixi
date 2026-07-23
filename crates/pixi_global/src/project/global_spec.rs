//! This module makes it a bit easier to pass around a package name and the pixi specification
use std::path::Path;

use pixi_manifest::{
    InlinePackageManifest, KnownPreviewFeature, Preview,
    toml::{TomlPackage, WorkspacePackageProperties},
};
use pixi_spec::PixiSpec;
use rattler_conda_types::{
    MatchSpec, NamelessMatchSpec, PackageName, ParseMatchSpecOptions, RepodataRevision,
};

/// The encapsulation of a package name and its associated
/// Pixi specification.
#[derive(Debug, Clone)]
pub struct GlobalSpec {
    pub name: PackageName,
    pub spec: PixiSpec,
    /// An inline package definition (`package = { ... }`) accompanying a
    /// source spec: it describes how the source is built when the source does
    /// not provide its own package manifest (or to override the one it has).
    pub inline: Option<InlinePackageValue>,
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum FromMatchSpecError {
    #[error("package name is required, not found for {0}")]
    NameRequired(Box<NamelessMatchSpec>),
    #[error(transparent)]
    ParseMatchSpec(#[from] rattler_conda_types::ParseMatchSpecError),
}

impl GlobalSpec {
    /// Creates a new `GlobalSpec` with a package name and a Pixi specification.
    pub fn new(name: PackageName, spec: PixiSpec) -> Self {
        Self {
            name,
            spec,
            inline: None,
        }
    }

    /// Attaches an inline package definition to this spec.
    pub fn with_inline(mut self, inline: InlinePackageValue) -> Self {
        self.inline = Some(inline);
        self
    }

    /// Returns the package name.
    pub fn name(&self) -> &PackageName {
        &self.name
    }

    /// Returns the Pixi specification.
    pub fn spec(&self) -> &PixiSpec {
        &self.spec
    }

    /// Convert from a &str and a ChannelConfig into a [`GlobalSpec`].
    pub fn try_from_str(
        spec_str: &str,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, FromMatchSpecError> {
        let match_spec = MatchSpec::from_str(
            spec_str,
            ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
        )?;
        GlobalSpec::try_from_matchspec_with_name(match_spec, channel_config)
    }

    /// Converts a [`MatchSpec`] into a [`GlobalSpec`].
    /// this can only result in a [`PixiSpec::DetailedVersion`] because
    /// a `MatchSpec` has no direct support for source specifications
    pub fn try_from_matchspec_with_name(
        match_spec: MatchSpec,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, FromMatchSpecError> {
        let (name_matcher, nameless_spec) = match_spec.into_nameless();
        let name = name_matcher
            .as_exact()
            .ok_or_else(|| FromMatchSpecError::NameRequired(Box::new(nameless_spec.clone())))?;
        let pixi_spec = PixiSpec::from_nameless_matchspec(nameless_spec, channel_config);
        Ok(GlobalSpec::new(name.clone(), pixi_spec))
    }
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum InlinePackageValueError {
    #[error("failed to parse the inline package definition: {0}")]
    Parse(String),
    #[error("an inline package definition cannot set `name`; it is taken from the dependency key")]
    ExplicitName,
    #[error(
        "an inline package definition cannot set `build.source`; the source is taken from the dependency spec"
    )]
    ExplicitBuildSource,
    #[error("failed to convert the inline package definition: {0}")]
    Convert(String),
}

/// An inline package definition as the TOML value of the `package` key of a
/// dependency entry (e.g. `package.build.backend.name = "pixi-build-rust"`).
///
/// The raw TOML representation is kept so the definition can be written
/// verbatim into the global manifest document; [`Self::to_inline_manifest`]
/// converts it into the resolved [`InlinePackageManifest`] when it is needed
/// before the manifest is saved and re-parsed (e.g. for package name
/// inference).
#[derive(Debug, Clone)]
pub struct InlinePackageValue(toml_edit::InlineTable);

impl InlinePackageValue {
    /// Wraps a `package` table.
    pub fn new(table: toml_edit::InlineTable) -> Self {
        Self(table)
    }

    /// Returns the definition as a TOML value, for insertion into a dependency
    /// entry under the `package` key.
    pub fn to_toml_value(&self) -> toml_edit::Value {
        toml_edit::Value::InlineTable(self.0.clone())
    }

    /// Parses and converts the definition into an [`InlinePackageManifest`]
    /// for the dependency `name`, mirroring the checks of the manifest parser:
    /// the definition may not set `name` (it comes from the dependency key)
    /// nor `build.source` (it comes from the dependency spec).
    pub fn to_inline_manifest(
        &self,
        name: &PackageName,
        root_directory: &Path,
    ) -> Result<InlinePackageManifest, InlinePackageValueError> {
        let source = format!("package = {}", self.0);
        let mut value =
            toml_span::parse(&source).map_err(|e| InlinePackageValueError::Parse(e.to_string()))?;
        let mut th = toml_span::de_helpers::TableHelper::new(&mut value)
            .map_err(|e| InlinePackageValueError::Parse(e.to_string()))?;
        let (_, mut package_value) = th
            .take("package")
            .expect("the `package` key was just serialized");
        let package = <TomlPackage as toml_span::Deserialize>::deserialize(&mut package_value)
            .map_err(|e| {
                InlinePackageValueError::Parse(
                    e.errors
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; "),
                )
            })?;

        if package.name.is_some() {
            return Err(InlinePackageValueError::ExplicitName);
        }
        if package.build.source.is_some() {
            return Err(InlinePackageValueError::ExplicitBuildSource);
        }

        let preview = Preview::from_iter([KnownPreviewFeature::PixiBuild]);
        InlinePackageManifest::from_toml_package(
            name,
            package,
            WorkspacePackageProperties::default(),
            &preview,
            root_directory,
        )
        .map(|with_warnings| with_warnings.value)
        .map_err(|e| InlinePackageValueError::Convert(e.to_string()))
    }
}
