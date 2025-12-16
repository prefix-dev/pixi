//! Protocol version abstraction for backwards compatibility.
//!
//! This module provides a trait-based abstraction over different versions of
//! the pixi build API protocol. This allows pixi to communicate with backends
//! using different protocol versions while maintaining type safety.
//!
//! # Protocol Versions
//!
//! - V1: Initial version with conda/outputs and conda/build_v1
//! - V2: Name in project models can be `None`
//! - V3: Outputs with the same name must have unique variants
//! - V4: SourcePackageSpec extended with version, build, build_number, etc.

use std::hash::Hash;

use serde::{Serialize, de::DeserializeOwned};

use crate::project_model::{SourcePackageSpecV1, SourcePackageSpecV4};

/// Trait that defines the types used for a specific protocol version.
///
/// Each protocol version can have different representations for certain types.
/// By implementing this trait for marker types (like `ApiV1`, `ApiV4`), we can
/// write generic code that works with any protocol version.
pub trait ProtocolVersion {
    /// The type used to represent a source package specification.
    type SourcePackageSpec: Serialize + DeserializeOwned + Clone + std::fmt::Debug + PartialEq + Eq + Hash;
}

/// Marker type for API version 1.
///
/// This version uses `SourcePackageSpecV1` which is a simple enum without
/// additional match spec fields.
#[derive(Debug, Clone, Copy)]
pub struct ApiV1;

impl ProtocolVersion for ApiV1 {
    type SourcePackageSpec = SourcePackageSpecV1;
}

/// Marker type for API version 2.
///
/// Same types as V1, but allows `None` for the name field in project models.
#[derive(Debug, Clone, Copy)]
pub struct ApiV2;

impl ProtocolVersion for ApiV2 {
    /// Unchanged from V1.
    type SourcePackageSpec = <ApiV1 as ProtocolVersion>::SourcePackageSpec;
}

/// Marker type for API version 3.
///
/// Same types as V2, but guarantees unique variants for outputs with same name.
#[derive(Debug, Clone, Copy)]
pub struct ApiV3;

impl ProtocolVersion for ApiV3 {
    /// Unchanged from V2.
    type SourcePackageSpec = <ApiV2 as ProtocolVersion>::SourcePackageSpec;
}

/// Marker type for API version 4.
///
/// This version uses `SourcePackageSpecV4` which is a struct with additional
/// match spec fields like `version`, `build`, `build_number`, etc.
#[derive(Debug, Clone, Copy)]
pub struct ApiV4;

impl ProtocolVersion for ApiV4 {
    type SourcePackageSpec = SourcePackageSpecV4;
}
