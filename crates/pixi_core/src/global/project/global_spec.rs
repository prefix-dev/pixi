//! This module makes it a bit easier to pass around a package name and the pixi specification
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, NamelessMatchSpec, PackageName, ParseStrictness};

/// The encapsulation of a package name and its associated
/// Pixi specification.
#[derive(Debug, Clone)]
pub enum GlobalSpec {
    // TODO: this will be used later
    #[allow(dead_code)]
    /// A global specification without a package name.
    /// can be a path or a URL.
    Nameless(PixiSpec),
    /// A global specification with a package name.
    Named(NamedGlobalSpec),
}

#[derive(Debug, Clone)]
pub struct NamedGlobalSpec {
    pub name: PackageName,
    pub spec: PixiSpec,
}

impl NamedGlobalSpec {
    /// Creates a new `NamedGlobalSpec` with a package name and a Pixi specification.
    pub fn new(name: PackageName, spec: PixiSpec) -> Self {
        Self { name, spec }
    }

    /// Returns the package name.
    pub fn name(&self) -> &PackageName {
        &self.name
    }

    /// Returns the Pixi specification.
    pub fn spec(&self) -> &PixiSpec {
        &self.spec
    }

    /// Convert from a &str and a ChannelConfig into a [`NamedGlobalSpec`].
    pub fn try_from_str(
        spec_str: &str,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, FromMatchSpecError> {
        let match_spec = MatchSpec::from_str(spec_str, ParseStrictness::Lenient)?;
        NamedGlobalSpec::try_from_matchspec_with_name(match_spec, channel_config)
    }

    /// Converts a [`MatchSpec`] into a [`GlobalSpec`].
    /// this can only result in a [`PixiSpec::Version`] or [`PixiSpec::DetailedVersion`] because
    /// a `MatchSpec` has no direct support for source specifications
    pub fn try_from_matchspec_with_name(
        match_spec: MatchSpec,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, FromMatchSpecError> {
        let (name, nameless_spec) = match_spec.into_nameless();
        if let Some(name) = name {
            let pixi_spec = PixiSpec::from_nameless_matchspec(nameless_spec, channel_config);
            Ok(NamedGlobalSpec::new(name, pixi_spec))
        } else {
            Err(FromMatchSpecError::NameRequired(Box::new(nameless_spec)))
        }
    }
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub enum FromMatchSpecError {
    #[error("package name is required, not found for {0}")]
    NameRequired(Box<NamelessMatchSpec>),
    #[error(transparent)]
    ParseMatchSpec(#[from] rattler_conda_types::ParseMatchSpecError),
}

impl GlobalSpec {
    /// Creates a new `GlobalSpec` without a package name.
    #[allow(dead_code)]
    pub fn nameless(spec: PixiSpec) -> Self {
        GlobalSpec::Nameless(spec)
    }

    /// Creates a new `GlobalSpec` with a package name and a Pixi specification.
    pub fn named(name: PackageName, spec: PixiSpec) -> Self {
        GlobalSpec::Named(NamedGlobalSpec { name, spec })
    }

    /// Convert from a &str and a ChannelConfig into a [`GlobalSpec`].
    /// If the spec contains a package name, it will create a Named variant,
    /// otherwise it will create a Nameless variant.
    pub fn try_from_str(
        spec_str: &str,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, FromMatchSpecError> {
        match NamedGlobalSpec::try_from_str(spec_str, channel_config) {
            Ok(named_spec) => Ok(GlobalSpec::Named(named_spec)),
            Err(FromMatchSpecError::NameRequired(nameless)) => Ok(GlobalSpec::Nameless(
                PixiSpec::from_nameless_matchspec(*nameless, channel_config),
            )),
            Err(e) => Err(e),
        }
    }

    pub fn into_named(self) -> Option<NamedGlobalSpec> {
        match self {
            GlobalSpec::Named(named_spec) => Some(named_spec),
            GlobalSpec::Nameless(_) => None,
        }
    }

    #[cfg(test)]
    pub fn spec(&self) -> &PixiSpec {
        match self {
            GlobalSpec::Nameless(spec) => spec,
            GlobalSpec::Named(named_spec) => &named_spec.spec,
        }
    }

    pub fn is_nameless(&self) -> bool {
        matches!(self, GlobalSpec::Nameless(_))
    }
}
