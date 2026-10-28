//! This module contains everything which is related to pixi global.

pub(crate) mod list;
pub use list::{GlobalDependency, GlobalEnvironment, GlobalExposedMapping};

// Reexport for pixi_api consumers
pub use pixi_global::*;
