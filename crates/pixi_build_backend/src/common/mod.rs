//! Common utilities that are shared between the different build backends.
mod configuration;
mod requirements;
mod variants;

pub use configuration::{BuildConfigurationParams, build_configuration};
pub use requirements::{PackageRequirements, SourceRequirements, requirements};
pub use variants::compute_variants;
