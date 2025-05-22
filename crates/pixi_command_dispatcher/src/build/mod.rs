//! Datastructures and functions used for building packages from source.

mod build_environment;
mod work_dir_key;

pub use build_environment::BuildEnvironment;
pub(crate) use work_dir_key::WorkDirKey;
