use crate::project::manifest::{FeatureName, TargetSelector};
use crate::project::SpecType;
use miette::Diagnostic;
use thiserror::Error;

/// An error that is returned when a certain spec is missing.
#[derive(Debug, Error, Diagnostic)]
#[error("{name} is missing")]
pub struct SpecIsMissing {
    // The name of the dependency that is missing,
    pub name: String,

    // The type of the dependency that is missing.
    pub spec_type: SpecType,

    /// Whether the dependency itself is missing or the entire dependency spec type is missing
    pub spec_type_is_missing: bool,

    // The target from which the dependency is missing.
    pub target: Option<TargetSelector>,

    // The feature from which the dependency is missing.
    pub feature: Option<FeatureName>,
}

impl SpecIsMissing {
    /// Constructs a new `SpecIsMissing` error that indicates that a spec type is missing.
    ///
    /// This is constructed for instance when the `[build-dependencies]` section is missing.
    pub fn spec_type_is_missing(name: impl Into<String>, spec_type: SpecType) -> Self {
        Self {
            name: name.into(),
            spec_type,
            spec_type_is_missing: true,
            target: None,
            feature: None,
        }
    }

    /// Constructs a new `SpecIsMissing` error that indicates that a spec is missing
    pub fn dep_is_missing(name: impl Into<String>, spec_type: SpecType) -> Self {
        Self {
            name: name.into(),
            spec_type,
            spec_type_is_missing: false,
            target: None,
            feature: None,
        }
    }

    /// Set the target from which the spec is missing.
    pub fn with_target(mut self, target: TargetSelector) -> Self {
        self.target = Some(target);
        self
    }

    /// Sets the feature from which the spec is missing.
    pub fn with_feature(mut self, feature: FeatureName) -> Self {
        self.feature = Some(feature);
        self
    }
}
