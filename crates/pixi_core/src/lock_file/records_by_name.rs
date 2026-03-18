use super::package_identifier::ConversionError;
use crate::lock_file::{LockedPypiRecord, PypiPackageIdentifier};
use pixi_install_pypi::UnresolvedPypiRecord;
use pixi_record::{PixiRecord, UnresolvedPixiRecord};
use pixi_uv_conversions::to_uv_normalize;
use pypi_modifiers::pypi_tags::is_python_record;
use rattler_conda_types::{PackageName, RepoDataRecord, VersionWithSource};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::hash::Hash;

pub type PypiRecordsByName = DependencyRecordsByName<UnresolvedPypiRecord>;
pub type LockedPypiRecordsByName = DependencyRecordsByName<LockedPypiRecord>;
pub type PixiRecordsByName = DependencyRecordsByName<PixiRecord>;
pub type UnresolvedPixiRecordsByName = DependencyRecordsByName<UnresolvedPixiRecord>;

/// A trait required from the dependencies stored in DependencyRecordsByName
pub trait HasNameVersion {
    // Name type of the dependency
    type N: Hash + Eq + Clone;
    // Version type of the dependency
    type V: PartialOrd + ToString;

    /// Returns the name of the dependency
    fn name(&self) -> &Self::N;
    /// Returns the version of the dependency, or `None` if the version is
    /// unknown (e.g. a pypi source dependency with a dynamic version).
    fn version(&self) -> Option<&Self::V>;
}

impl HasNameVersion for LockedPypiRecord {
    type N = pep508_rs::PackageName;
    type V = pep440_rs::Version;

    fn name(&self) -> &pep508_rs::PackageName {
        &self.data.name
    }
    fn version(&self) -> Option<&Self::V> {
        Some(&self.locked_version)
    }
}

impl HasNameVersion for UnresolvedPypiRecord {
    type N = pep508_rs::PackageName;
    type V = pep440_rs::Version;

    fn name(&self) -> &pep508_rs::PackageName {
        &self.as_package_data().name
    }
    fn version(&self) -> Option<&Self::V> {
        self.as_package_data().version.as_ref()
    }
}

impl HasNameVersion for RepoDataRecord {
    type N = rattler_conda_types::PackageName;
    type V = VersionWithSource;

    fn name(&self) -> &rattler_conda_types::PackageName {
        &self.package_record.name
    }
    fn version(&self) -> Option<&Self::V> {
        Some(&self.package_record.version)
    }
}

impl HasNameVersion for PixiRecord {
    type N = PackageName;
    type V = VersionWithSource;

    fn name(&self) -> &Self::N {
        &self.package_record().name
    }

    fn version(&self) -> Option<&Self::V> {
        Some(&self.package_record().version)
    }
}

impl HasNameVersion for UnresolvedPixiRecord {
    type N = PackageName;
    type V = VersionWithSource;

    fn name(&self) -> &Self::N {
        UnresolvedPixiRecord::name(self)
    }

    fn version(&self) -> Option<&Self::V> {
        self.package_record().map(|pr| &pr.version)
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
                    // Use the entry with the highest version or otherwise the first
                    // we encounter. If either version is `None` (e.g. a pypi source
                    // dependency with a dynamic version), keep the existing entry.
                    let idx = *entry.get();
                    if let (Some(existing), Some(new)) = (records[idx].version(), record.version())
                        && existing < new
                    {
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
                    PypiPackageIdentifier::from_package_record(source_record.package_record())
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

impl UnresolvedPixiRecordsByName {
    /// Converts to a [`PixiRecordsByName`] on a best-effort basis.
    ///
    /// Binary records and full source records are converted; partial source
    /// records (whose metadata is incomplete) are silently dropped.
    pub(crate) fn into_resolved_best_effort(self) -> PixiRecordsByName {
        PixiRecordsByName::from_iter(
            self.records
                .into_iter()
                .filter_map(|r| r.try_into_resolved().ok()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_lock::{PypiPackageData, UrlOrPath, Verbatim};
    use std::str::FromStr;

    fn make_pypi_package(name: &str, version: Option<&str>) -> PypiPackageData {
        PypiPackageData {
            name: name.parse().unwrap(),
            version: version.map(|v| pep440_rs::Version::from_str(v).unwrap()),
            location: Verbatim::new(UrlOrPath::Path(format!("./{name}").into())),
            hash: None,
            index_url: None,
            requires_dist: vec![],
            requires_python: None,
        }
    }

    #[test]
    fn from_iter_with_none_version_does_not_panic() {
        // A single package with no version should work fine.
        let records = vec![make_pypi_package("dynamic-dep", None).into()];
        let by_name = PypiRecordsByName::from_iter(records);
        assert_eq!(by_name.len(), 1);
        assert!(by_name.records[0].version().is_none());
    }

    #[test]
    fn from_iter_dedup_keeps_first_when_both_versions_none() {
        // Two packages with the same name and no version — should keep the first.
        let records = vec![
            make_pypi_package("dynamic-dep", None).into(),
            make_pypi_package("dynamic-dep", None).into(),
        ];
        let by_name = PypiRecordsByName::from_iter(records);
        assert_eq!(by_name.len(), 1);
        assert!(by_name.records[0].version().is_none());
    }

    #[test]
    fn from_iter_dedup_keeps_first_when_existing_has_no_version() {
        // First entry has no version, second has a version — keeps the first
        // because we can't compare None to Some.
        let records = vec![
            make_pypi_package("pkg", None).into(),
            make_pypi_package("pkg", Some("1.0.0")).into(),
        ];
        let by_name = PypiRecordsByName::from_iter(records);
        assert_eq!(by_name.len(), 1);
        assert!(by_name.records[0].version().is_none());
    }

    #[test]
    fn from_iter_dedup_keeps_first_when_new_has_no_version() {
        // First entry has a version, second has no version — keeps the first.
        let records = vec![
            make_pypi_package("pkg", Some("1.0.0")).into(),
            make_pypi_package("pkg", None).into(),
        ];
        let by_name = PypiRecordsByName::from_iter(records);
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name.records[0].version().unwrap().to_string(), "1.0.0");
    }

    #[test]
    fn from_iter_dedup_picks_higher_version() {
        let records = vec![
            make_pypi_package("pkg", Some("1.0.0")).into(),
            make_pypi_package("pkg", Some("2.0.0")).into(),
        ];
        let by_name = PypiRecordsByName::from_iter(records);
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name.records[0].version().unwrap().to_string(), "2.0.0");
    }

    #[test]
    fn from_unique_iter_with_none_version() {
        // from_unique_iter should work fine with None version (it doesn't compare versions).
        let records = vec![make_pypi_package("dynamic-dep", None).into()];
        let by_name = PypiRecordsByName::from_unique_iter(records).unwrap();
        assert_eq!(by_name.len(), 1);
        assert!(by_name.records[0].version().is_none());
    }

    #[test]
    fn mixed_versioned_and_dynamic_packages() {
        let records = vec![
            make_pypi_package("versioned-pkg", Some("1.0.0")).into(),
            make_pypi_package("dynamic-pkg", None).into(),
        ];
        let by_name = PypiRecordsByName::from_iter(records);
        assert_eq!(by_name.len(), 2);

        let versioned = by_name.by_name(&"versioned-pkg".parse().unwrap()).unwrap();
        assert_eq!(versioned.version().unwrap().to_string(), "1.0.0");

        let dynamic = by_name.by_name(&"dynamic-pkg".parse().unwrap()).unwrap();
        assert!(dynamic.version().is_none());
    }
}
