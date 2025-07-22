//! This module determines what actions should be taken when installing
//! or updating Python packages from PyPI into a Conda environment. It handles:
//!
//! - Determining which packages need to be installed, reinstalled, or removed
//! - Deciding whether packages should come from local cache or remote sources
//! - Validating existing installations against locked requirements
//! - Avoiding unnecessary downloads when possible
//!
//! The core types include:
//! - [`PixiInstallPlan`]: The final plan of installation operations
//! - [`InstallPlanner`]: Builds installation plans from the current state
//! - [`InstallReason`]: Why a specific package is being installed
//! - [`NeedReinstall`]: Why a package needs reinstallation
//!
//! ## Getting Started
//! Start with the [`PixiInstallPlan`] struct to understand the planning output,
//! then explore [`InstallPlanner`] to see how these plans are built. The
//! `planner.rs` file contains the main coordination logic.
//!
//! ## How Installation Planning Works
//! An installation plan is built through these steps:
//!
//! 1. Examine all currently installed packages in the environment
//! 2. For each installed package, determine if it's:
//!    - Required but needs reinstallation
//!    - Required and can be kept as-is
//!    - No longer required (extraneous)
//! 3. For packages needing installation, determine if they can come from
//!    local cache or must be downloaded
//! 4. Check for any required packages that aren't yet installed
//!
//! The result categorizes all packages into those installable from cache,
//! those needing download, those requiring reinstallation, and those to be removed.
//!
//! This module builds on UV's distribution handling while applying Pixi-specific
//! customizations for managing Python packages in Conda environments.
mod cache;
mod installation_source;
mod installed_dists;
mod models;
mod planner;
mod reasons;
mod required_dists;
mod validation;

pub use cache::CachedWheels;
pub(crate) use models::NeedReinstall;
pub use models::PyPIInstallationPlan;
pub use planner::InstallPlanner;
pub use reasons::InstallReason;
pub use required_dists::RequiredDists;

#[cfg(test)]
mod test;
