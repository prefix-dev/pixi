//! Concrete purl derivation resolvers.
//!
//! Each module corresponds to one [`crate::PurlDerivationSource`] variant:
//!
//! - [`ProjectDefinedMapping`] derives from project/user-defined per-channel mappings.
//! - [`PrefixHashResolver`] derives from prefix.dev hash mappings keyed by package SHA256.
//! - [`PrefixCompressedResolver`] derives from prefix.dev compressed name mappings.
//! - `CondaForgeVerbatim` derives by assuming conda-forge package names are PyPI names.

mod conda_forge_verbatim;
mod prefix_compressed_resolver;
mod prefix_hash_resolver;
mod project_defined_mapping;

pub(crate) use conda_forge_verbatim::CondaForgeVerbatim;
pub use prefix_compressed_resolver::{PrefixCompressedResolver, PrefixCompressedResolverBuilder};
pub use prefix_hash_resolver::{
    PrefixHashResolver, PrefixHashResolverBuilder, PrefixHashResolverError,
};
pub use project_defined_mapping::ProjectDefinedMapping;
pub(crate) use project_defined_mapping::ProjectDefinedResolver;
