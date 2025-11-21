//! Development source records.
//!
//! This module defines the record type for development sources - source packages
//! whose dependencies are installed without building the package itself.

use std::collections::BTreeMap;

use pixi_spec::{BinarySpec, PixiSpec, SourceSpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::PackageName;

use itertools::{Either, Itertools};

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

impl DevSourceRecord {
    /// Returns an iterator over all dependencies from dev source records,
    /// excluding packages that are themselves dev sources.
    pub fn dev_source_dependencies(
        dev_source_records: &[DevSourceRecord],
    ) -> impl Iterator<Item = (rattler_conda_types::PackageName, PixiSpec)> + '_ {
        use std::collections::HashSet;

        // Collect all dev source package names to filter them out
        let dev_source_names: HashSet<_> = dev_source_records
            .iter()
            .map(|record| record.name.clone())
            .collect();

        // Collect all dependencies from all dev sources, filtering out dev sources themselves
        dev_source_records
            .iter()
            .flat_map(|dev_source| {
                dev_source
                    .dependencies
                    .iter_specs()
                    .map(|(name, spec)| (name.clone(), spec.clone()))
                    .collect::<Vec<_>>()
            })
            .filter(move |(name, _)| !dev_source_names.contains(name))
    }

    /// Split the set of requirements into source and binary requirements.
    pub fn split_into_source_and_binary_requirements(
        specs: impl IntoIterator<Item = (rattler_conda_types::PackageName, PixiSpec)>,
    ) -> (
        DependencyMap<rattler_conda_types::PackageName, SourceSpec>,
        DependencyMap<rattler_conda_types::PackageName, BinarySpec>,
    ) {
        specs.into_iter().partition_map(|(name, constraint)| {
            match constraint.into_source_or_binary() {
                Either::Left(source) => Either::Left((name, source)),
                Either::Right(binary) => Either::Right((name, binary)),
            }
        })
    }
}
