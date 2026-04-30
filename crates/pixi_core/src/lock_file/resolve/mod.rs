//! This module contains code to resolve python package from PyPi or Conda packages.
//!
//! See [`pypi::resolve_pypi`] for more information.

pub(crate) mod build_dispatch;
pub(crate) mod pypi;
mod resolver_provider;
