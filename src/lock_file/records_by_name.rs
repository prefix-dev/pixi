use crate::lock_file::{PypiPackageIdentifier, PypiRecord};
use crate::pypi_tags::is_python_record;
use rattler_conda_types::{PackageName, RepoDataRecord};
use std::borrow::Borrow;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::hash::Hash;

/// A struct that holds both a ``Vec` of `RepoDataRecord` and a mapping from name to index.
#[derive(Clone, Debug, Default)]
pub struct RepoDataRecordsByName {
    pub records: Vec<RepoDataRecord>,
    by_name: HashMap<PackageName, usize>,
}

impl From<Vec<RepoDataRecord>> for RepoDataRecordsByName {
    fn from(records: Vec<RepoDataRecord>) -> Self {
        let by_name = records
            .iter()
            .enumerate()
            .map(|(idx, record)| (record.package_record.name.clone(), idx))
            .collect();
        Self { records, by_name }
    }
}

impl RepoDataRecordsByName {
    /// Returns the record with the given name or `None` if no such record exists.
    pub fn by_name<Q: ?Sized>(&self, key: &Q) -> Option<&RepoDataRecord>
    where
        PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Returns the index of the record with the given name or `None` if no such record exists.
    pub fn index_by_name<Q: ?Sized>(&self, key: &Q) -> Option<usize>
    where
        PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).copied()
    }

    /// Returns the record that represents the python interpreter or `None` if no such record exists.
    pub fn python_interpreter_record(&self) -> Option<&RepoDataRecord> {
        self.records.iter().find(|record| is_python_record(*record))
    }

    /// Returns true if there are no records stored in this instance
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns the number of entries in the mapping.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Converts this instance into the internally stored records.
    pub fn into_inner(self) -> Vec<RepoDataRecord> {
        self.records
    }

    /// Constructs a new instance from an iterator of repodata records. If multiple records exist
    /// for the same package name an error is returned.
    pub fn from_unique_iter<I: IntoIterator<Item = RepoDataRecord>>(
        iter: I,
    ) -> Result<Self, Box<RepoDataRecord>> {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.package_record.name.clone()) {
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

    /// Constructs a new instance from an iterator of repodata records. The records are
    /// deduplicated where the record with the highest version wins.
    pub fn from_iter<I: IntoIterator<Item = RepoDataRecord>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.package_record.name.clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if records[idx].package_record.version < record.package_record.version {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }

    /// Convert the records into a map of pypi package identifiers mapped to the records they were
    /// extracted from.
    pub fn by_pypi_name(
        &self,
    ) -> HashMap<uv_normalize::PackageName, (PypiPackageIdentifier, usize, &RepoDataRecord)> {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(idx, record)| {
                PypiPackageIdentifier::from_record(record)
                    .ok()
                    .map(move |identifiers| (idx, record, identifiers))
            })
            .flat_map(|(idx, record, identifiers)| {
                identifiers.into_iter().map(move |identifier| {
                    (
                        identifier.name.as_normalized().clone(),
                        (identifier, idx, record),
                    )
                })
            })
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
pub struct PypiRecordsByName {
    pub records: Vec<PypiRecord>,
    by_name: HashMap<uv_normalize::PackageName, usize>,
}

impl PypiRecordsByName {
    /// Returns the record with the given name or `None` if no such record exists.
    pub fn by_name<Q: ?Sized>(&self, key: &Q) -> Option<&PypiRecord>
    where
        uv_normalize::PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).map(|idx| &self.records[*idx])
    }

    /// Returns the index of the record with the given name or `None` if no such record exists.
    pub fn index_by_name<Q: ?Sized>(&self, key: &Q) -> Option<usize>
    where
        uv_normalize::PackageName: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.by_name.get(key).copied()
    }

    /// Returns true if there are no records stored in this instance
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Returns the number of entries in the mapping.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns an iterator over the names of the records stored in this instance.
    pub fn names(&self) -> impl Iterator<Item = &uv_normalize::PackageName> {
        self.by_name.keys()
    }

    /// Converts this instance into the internally stored records.
    pub fn into_inner(self) -> Vec<PypiRecord> {
        self.records
    }

    /// Constructs a new instance from an iterator of pypi records. If multiple records exist
    /// for the same package name an error is returned.
    pub fn from_unique_iter<I: IntoIterator<Item = PypiRecord>>(
        iter: I,
    ) -> Result<Self, PypiRecord> {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.0.name.clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(_) => {
                    return Err(record);
                }
            }
        }
        Ok(Self { records, by_name })
    }

    /// Constructs a new instance from an iterator of repodata records. The records are
    /// deduplicated where the record with the highest version wins.
    pub fn from_iter<I: IntoIterator<Item = PypiRecord>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let min_size = iter.size_hint().0;
        let mut by_name = HashMap::with_capacity(min_size);
        let mut records = Vec::with_capacity(min_size);
        for record in iter {
            match by_name.entry(record.0.name.clone()) {
                Entry::Vacant(entry) => {
                    let idx = records.len();
                    records.push(record);
                    entry.insert(idx);
                }
                Entry::Occupied(entry) => {
                    // Use the entry with the highest version or otherwise the first we encounter.
                    let idx = *entry.get();
                    if records[idx].0.version < record.0.version {
                        records[idx] = record;
                    }
                }
            }
        }

        Self { records, by_name }
    }
}
