//! This module makes it a bit easier to pass around a package name and the pixi specification
use pixi_spec::PixiSpec;
use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness};

/// The encapsulation of a package name and its associated
/// Pixi specification.
#[derive(Debug, Clone)]
pub enum GlobalSpec {
    /// A global specification without a package name.
    /// can be a path or a URL.
    Nameless(PixiSpec),
    /// A global specification with a package name.
    Named(NamedGlobalSpec),
}

#[derive(Debug, Clone)]
pub struct NamedGlobalSpec {
    name: PackageName,
    spec: PixiSpec,
}

impl NamedGlobalSpec {
    /// Convert from a &str and a ChannelConfig into a [`NamedGlobalSpec`].
    pub fn from_str(
        spec_str: &str,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, FromMatchSpecError> {
        let match_spec = MatchSpec::from_str(spec_str, ParseStrictness::Lenient)?;
        NamedGlobalSpec::from_matchspec_with_name(match_spec, channel_config)
    }

    /// Converts a [`MatchSpec`] into a [`GlobalSpec`].
    /// this can only result in a [`PixiSpec::Version`] or [`PixiSpec::DetailedVersion`] because
    /// a `MatchSpec` has no direct support for source specifications
    pub fn from_matchspec_with_name(
        match_spec: MatchSpec,
        channel_config: &rattler_conda_types::ChannelConfig,
    ) -> Result<Self, FromMatchSpecError> {
        let (name, nameless_spec) = match_spec.into_nameless();
        if let Some(name) = name {
            let pixi_spec = PixiSpec::from_nameless_matchspec(nameless_spec, channel_config);
            Ok(NamedGlobalSpec::new(name, pixi_spec))
        } else {
            Err(FromMatchSpecError::NameRequired(nameless_spec.to_string()))
        }
    }
}

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
enum FromMatchSpecError {
    #[error("package name is required, not found for {0}")]
    NameRequired(String),
    #[error(transparent)]
    ParseMatchSpec(#[from] rattler_conda_types::ParseMatchSpecError),
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

    /// Converts into tuple of (name, spec).
    pub fn into_tuple(self) -> (PackageName, PixiSpec) {
        (self.name, self.spec)
    }
}

impl GlobalSpec {
    /// Creates a new `GlobalSpec` without a package name.
    pub fn nameless(spec: PixiSpec) -> Self {
        GlobalSpec::Nameless(spec)
    }

    /// Creates a new `GlobalSpec` with a package name and a Pixi specification.
    pub fn named(name: PackageName, spec: PixiSpec) -> Self {
        GlobalSpec::Named(NamedGlobalSpec { name, spec })
    }

    /// Returns the package name of the global spec if it has one.
    pub fn name(&self) -> Option<&PackageName> {
        match self {
            GlobalSpec::Named(named_spec) => Some(&named_spec.name),
            GlobalSpec::Nameless(_) => None,
        }
    }

    /// Returns the Pixi specification of the global spec.
    pub fn pixi_spec(&self) -> &PixiSpec {
        match self {
            GlobalSpec::Named(named_spec) => &named_spec.spec,
            GlobalSpec::Nameless(spec) => spec,
        }
    }

    /// Converts the `GlobalSpec` into a tuple containing the optional package name and Pixi specification.
    pub fn into_tuple(self) -> (Option<PackageName>, PixiSpec) {
        match self {
            GlobalSpec::Named(named_spec) => (Some(named_spec.name), named_spec.spec),
            GlobalSpec::Nameless(spec) => (None, spec),
        }
    }

    /// Converts a reference to the `GlobalSpec` into a tuple containing references to the optional package name and Pixi specification.
    pub fn into_tuple_ref(&self) -> (Option<&PackageName>, &PixiSpec) {
        match self {
            GlobalSpec::Named(named_spec) => (Some(&named_spec.name), &named_spec.spec),
            GlobalSpec::Nameless(spec) => (None, spec),
        }
    }

    /// Returns the named global spec if this is a named variant.
    pub fn as_named(&self) -> Option<&NamedGlobalSpec> {
        match self {
            GlobalSpec::Named(named_spec) => Some(named_spec),
            GlobalSpec::Nameless(_) => None,
        }
    }

    pub fn into_named(self) -> Option<NamedGlobalSpec> {
        match self {
            GlobalSpec::Named(named_spec) => Some(named_spec),
            GlobalSpec::Nameless(_) => None,
        }
    }

    /// Returns the named global spec if this is a named variant.
    pub fn as_nameless(&self) -> Option<&PixiSpec> {
        match self {
            GlobalSpec::Named(_) => None,
            GlobalSpec::Nameless(spec) => Some(spec),
        }
    }

    pub fn into_nameless(self) -> Option<PixiSpec> {
        match self {
            GlobalSpec::Named(_) => None,
            GlobalSpec::Nameless(spec) => Some(spec),
        }
    }
}
