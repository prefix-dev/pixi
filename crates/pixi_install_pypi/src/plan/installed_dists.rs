//! Defines provider traits for accessing installed Python packages.
//!
//! This module contains the `InstalledDistProvider` trait which provides
//! iteration over installed Python distributions. This trait enables abstraction
//! over package installation operations and supports mocking for testing purposes.
//!
use uv_distribution_types::InstalledDist;
use uv_installer::SitePackages;

// Below we define a couple of traits so that we can make the creation of the install plan
// somewhat more abstract
//
/// Provide an iterator over the installed distributions
/// This trait can also be used to mock the installed distributions for testing purposes
pub trait InstalledDists<'a> {
    /// Provide an iterator over the installed distributions
    fn iter(&'a self) -> impl Iterator<Item = &'a InstalledDist>;
}

impl<'a> InstalledDists<'a> for SitePackages {
    fn iter(&'a self) -> impl Iterator<Item = &'a InstalledDist> {
        self.iter()
    }
}
