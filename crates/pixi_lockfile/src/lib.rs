use pixi_record::PixiRecord;
use rattler_lock::PypiPackageData;
use rattler_lock::PypiPackageEnvironmentData;

mod package_identifier;
mod records_by_name;
mod resolve;
pub mod satisfiability;
pub mod virtual_packages;

pub use package_identifier::{ConversionError, PypiPackageIdentifier};
pub use records_by_name::{HasNameVersion, PixiRecordsByName, PypiRecordsByName};
pub use resolve::uv_resolution_context::UvResolutionContext;

/// A list of conda packages that are locked for a specific platform.
pub type LockedCondaPackages = Vec<PixiRecord>;

/// A list of Pypi packages that are locked for a specific platform.
pub type LockedPypiPackages = Vec<PypiRecord>;

/// A single Pypi record that contains both the package data and the environment
/// data. In Pixi we basically always need both.
pub type PypiRecord = (PypiPackageData, PypiPackageEnvironmentData);
