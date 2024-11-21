//! Implementations of the [`crate::Protocol`] type for various backends.

pub(super) mod conda_build;
mod error;
pub(super) mod pixi;
pub(super) mod rattler_build;

// pub trait DiscoverableProtocolBuilder {
//     fn discover(source_dir: &Path) -> Result<Option<Self>, error::DiscoveryError>
//     where
//         Self: Sized;
// }
