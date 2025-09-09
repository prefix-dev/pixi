use super::package_identifier::ConversionError;
use crate::lock_file::{PypiPackageIdentifier, PypiRecord};
use pixi_record::PixiRecord;
use pixi_uv_conversions::to_uv_normalize;
use pypi_modifiers::pypi_tags::is_python_record;
use rattler_conda_types::{PackageName, RepoDataRecord, VersionWithSource};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::Hash;

pub type PypiRecordsByName = DependencyRecordsByName<PypiRecord>;
pub type PixiRecordsByName = DependencyRecordsByName<PixiRecord>;

/// A trait required from the dependencies stored in DependencyRecordsByName
pub trait HasNameVersion {
    // Name type of the dependency
    type N: Hash + Eq + Clone;
    // Version type of the dependency
    type V: PartialOrd + ToString;

    /// Returns the name of the dependency
    fn name(&self) -> &Self::N;
    /// Returns the version of the dependency
    fn version(&self) -> &Self::V;
}

impl HasNameVersion for PypiRecord {
    type N = pep508_rs::PackageName;
    type V = pep440_rs::Version;

    fn name(&self) -> &pep508_rs::PackageName {
        &self.0.name
    }
    fn version(&self) -> &Self::V {
        &self.0.version
    }
}

impl HasNameVersion for RepoDataRecord {
    type N = rattler_conda_types::PackageName;
    type V = VersionWithSource;

    fn name(&self) -> &rattler_conda_types::PackageName {
        &self.package_record.name
    }
    fn version(&self) -> &Self::V {
        &self.package_record.version
    }
}

impl HasNameVersion for PixiRecord {
    type N = PackageName;
    type V = VersionWithSource;

    fn name(&self) -> &Self::N {
        &self.package_record().name
    }

    fn version(&self) -> &Self::V {
        &self.package_record().version
    }
}

/// A struct that holds both a ``Vec` of `DependencyRecord` and a mapping from
/// name to index.
#[derive(Clone, Debug)]
pub struct DependencyRecordsByName<D: HasNameVersion> {
    pub records: Vec<D>,
    by_name: HashMap<D::N, usize>,
}

impl<D: HasNameVersion> Default for DependencyRecordsByName<D> {
    fn default() -> Self {
        Self {
            records: Vec::new(),
            by_name: HashMap::new(),
        }
    }
}

impl<D: HasNameVersion> From<Vec<D>> for DependencyRecordsByName<D> {
    fn from(records: Vec<D>) -> Self {
        let by_name = records
            .iter()
            .enumerate()
            .map(|(idx, record)| (record.name().clone(), idx))
            .collect();
        Self { records, by_name }
    }
}

impl<D: HasNameVersion> DependencyRecordsByName<D> {
    /// Returns the record with the given name or `None` if no such record
    /// exists.
    pub(crate) fn by_name(&self, key: &D::N) -> Option<&D> {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Returns the index of the record with the given name or `None` if no such
    /// record exists.
    pub(crate) fn index_by_name(&self, key: &D::N) -> Option<usize> {
        self.by_name.get(key).copied()
    }
    /// Returns true if there are no records stored in this instance
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns the number of entries in the mapping.
    pub(crate) fn len(&self) -> usize {
        self.records.len()
    }

    /// Converts this instance into the internally stored records.
    pub(crate) fn into_inner(self) -> Vec<D> {
        self.records
    }

    /// Returns an iterator over the names of the records stored in this
    /// instance.
    pub(crate) fn names(&self) -> impl Iterator<Item = &D::N> {
        // Iterate over the records to retain the index of the original record.
        self.records.iter().map(|r| r.name())
    }

    /// Constructs a new instance from an iterator of pypi records. If multiple
    /// records exist for the same package name an error is returned.
    pub(crate) fn from_unique_iter<I: IntoIterator<Item = D>>(iter: I) -> Result<Self, Box<D>> {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.name().clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(_) => {
                    return Err(Box::new(record));
                }
            }
        }
        Ok(Self { records, by_name })
    }

    /// Constructs a new instance from an iterator of repodata records. The
    /// records are deduplicated where the record with the highest version
    /// wins.
    pub(crate) fn from_iter<I: IntoIterator<Item = D>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.name().clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if records[idx].version() < record.version() {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }
}

impl PixiRecordsByName {
    /// Returns the record that represents the python interpreter or `None` if
    /// no such record exists.
    pub(crate) fn python_interpreter_record(&self) -> Option<&RepoDataRecord> {
        self.records.iter().find_map(|record| match record {
            PixiRecord::Binary(record) if is_python_record(record) => Some(record),
            _ => None,
        })
    }

    /// Convert the records into a map of pypi package identifiers mapped to the
    /// records they were extracted from.
    pub(crate) fn by_pypi_name(
        &self,
    ) -> Result<
        HashMap<uv_normalize::PackageName, (PypiPackageIdentifier, usize, &PixiRecord)>,
        ConversionError,
    > {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(idx, record)| match record {
                PixiRecord::Binary(repodata_record) => {
                    PypiPackageIdentifier::from_repodata_record(repodata_record)
                        .ok()
                        .map(move |identifiers| (idx, record, identifiers))
                }
                PixiRecord::Source(source_record) => {
                    PypiPackageIdentifier::from_package_record(&source_record.package_record)
                        .ok()
                        .map(move |identifiers| (idx, record, identifiers))
                }
            })
            .flat_map(|(idx, record, identifiers)| {
                identifiers.into_iter().map(move |identifier| {
                    let name = to_uv_normalize(identifier.name.as_normalized())?;
                    Ok((name, (identifier, idx, record)))
                })
            })
            .collect::<Result<HashMap<_, _>, ConversionError>>()
    }
}
