//! This module contains code to resolve python package from PyPi or Conda packages.
//!
//! See [`resolve_pypi`] and [`resolve_conda`] for more information.

pub(crate) mod build_dispatch;
pub(crate) mod pypi;
mod resolver_provider;
pub(crate) mod uv_resolution_context;
