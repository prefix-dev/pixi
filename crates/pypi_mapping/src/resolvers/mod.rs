//! Concrete purl derivation resolvers.
//!
//! Each module corresponds to one [`crate::PurlDerivationSource`] variant:
//!
//! - [`ProjectDefinedMapping`] derives from project/user-defined per-channel mappings.
//! - [`PrefixHash`] derives from prefix.dev hash mappings keyed by package SHA256.
//! - [`PrefixCompressed`] derives from prefix.dev compressed name mappings.
//! - `SameName` derives by assuming conda package names are PyPI names.

mod prefix_compressed;
mod prefix_hash;
mod project_defined;
mod same_name;

pub use prefix_compressed::{PrefixCompressed, PrefixCompressedBuilder};
pub use prefix_hash::{PrefixHash, PrefixHashBuilder, PrefixHashError};
pub(crate) use project_defined::ProjectDefined;
pub use project_defined::ProjectDefinedMapping;
pub(crate) use same_name::SameName;
