//! Environment input layer. Outer Keys carry an [`EnvironmentRef`] that
//! resolves through [`WorkspaceEnvRegistry`] to an immutable
//! [`EnvironmentSpec`]; field-level projection Keys ([`ChannelsOf`],
//! [`BuildEnvOf`], etc.) fan individual fields out to their consumers.
//! Cross-env cache sharing happens at deeper content-hashed layers.

mod env_ref;
mod projections;
mod registry;
mod spec;
mod workspace_env_ref;

pub use env_ref::{DerivedEnvKind, DerivedParent, EnvironmentRef, EphemeralEnv};
pub use projections::{BuildEnvOf, ChannelsOf, ExcludeNewerOf, VariantsOf};
pub use registry::{HasWorkspaceEnvRegistry, WorkspaceEnvRegistry};
pub use spec::EnvironmentSpec;
pub use workspace_env_ref::{WorkspaceEnvId, WorkspaceEnvRef};
