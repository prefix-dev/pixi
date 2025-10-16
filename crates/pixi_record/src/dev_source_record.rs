//! Development source records.
//!
//! This module defines the record type for development sources - source packages
//! whose dependencies are installed without building the package itself.

use std::collections::BTreeMap;

use pixi_spec::{BinarySpec, PixiSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::PackageName;

use crate::{InputHash, PinnedSourceSpec};

/// A resolved development source record representing a specific output from a source package.
///
/// This contains all the metadata needed by the solver to select and use this output.
/// Unlike `SourceRecord`, this represents a "virtual" package where only the dependencies
/// are installed, not the package itself.
#[derive(Debug, Clone)]
pub struct DevSourceRecord {
    /// The name of the package/output
    pub name: PackageName,

    /// The pinned source this record came from
    pub source: PinnedSourceSpec,

    /// Hash of input files used to generate this metadata
    pub input_hash: Option<InputHash>,

    /// Variants used when computing dependencies. This is used to uniquely identify this record.
    pub variants: BTreeMap<String, String>,

    /// All dependencies (build, host, and run combined)
    pub dependencies: DependencyMap<PackageName, PixiSpec>,

    /// All constraints combined
    pub constraints: DependencyMap<PackageName, BinarySpec>,
}
