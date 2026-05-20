mod canonical_spec;
mod dev_source_record;
mod lock_file_resolver;
mod pinned_source;
mod source_record;

pub use canonical_spec::{CanonicalGit, CanonicalPath, CanonicalSourceLocation, CanonicalUrl};
pub use dev_source_record::DevSourceRecord;
pub use lock_file_resolver::{LockFileResolver, LockFileResolverError};

use std::{collections::HashMap, path::Path, sync::Arc};

pub use pinned_source::{
    LockedGitUrl, MutablePinnedSourceSpec, ParseError, PinnedGitCheckout, PinnedGitSpec,
    PinnedPathSpec, PinnedSourceSpec, PinnedUrlSpec, SourceMismatchError,
};
pub use pixi_variant::VariantValue;
use rattler_conda_types::{
    MatchSpec, Matches, NamelessMatchSpec, PackageName, PackageRecord, RepoDataRecord,
};
use rattler_lock::{
    CondaPackageData, ConversionError, EnvironmentPackages, LockFileBuilder, PackageHandle,
    SourceData, UrlOrPath,
};
use serde::Serialize;
pub use source_record::{
    FullSourceRecord as SourceRecord, FullSourceRecordData, PartialSourceRecord,
    PartialSourceRecordData, PinnedBuildSourceSpec, SourceRecordData, UnresolvedSourceRecord,
};
use thiserror::Error;

/// A record of a conda package that is either something installable from a
/// binary file or something that still requires building.
///
/// This is basically a superset of a regular [`RepoDataRecord`].
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize)]
#[serde(untagged)]
pub enum PixiRecord {
    Binary(Arc<RepoDataRecord>),
    Source(Arc<SourceRecord>),
}
impl PixiRecord {
    /// The name of the package
    pub fn name(&self) -> &PackageName {
        &self.package_record().name
    }

    /// Metadata information of the package.
    pub fn package_record(&self) -> &PackageRecord {
        match self {
            PixiRecord::Binary(record) => &record.package_record,
            PixiRecord::Source(record) => &record.data.package_record,
        }
    }

    /// Convert to `CondaPackageData` with paths made relative to
    /// `workspace_root`. For source records, each entry of `build_packages` /
    /// `host_packages` is registered into the writer's builder first
    /// (recursively) so the returned source data's `source_data` references
    /// them by handle. The writer's cache memoizes shared build/host
    /// subtrees across calls; see [`LockFileWriter`].
    pub fn into_conda_package_data(
        self,
        writer: &mut LockFileWriter<'_>,
        workspace_root: &Path,
    ) -> CondaPackageData {
        match self {
            PixiRecord::Binary(record) => Arc::unwrap_or_clone(record).into(),
            PixiRecord::Source(record) => {
                let mut source = Arc::unwrap_or_clone(record);
                let source_data = register_source_deps(writer, &mut source, workspace_root);
                let mut data = source.into_conda_source_data(workspace_root);
                data.source_data = source_data;
                CondaPackageData::Source(Box::new(data))
            }
        }
    }

    /// Returns a reference to the binary record if it is a binary record.
    pub fn as_binary(&self) -> Option<&RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    /// Converts this instance into a binary record if it is a binary record.
    pub fn into_binary(self) -> Option<Arc<RepoDataRecord>> {
        match self {
            PixiRecord::Binary(record) => Some(record),
            PixiRecord::Source(_) => None,
        }
    }

    /// Converts this instance into a source record if it is a source
    pub fn into_source(self) -> Option<Arc<SourceRecord>> {
        match self {
            PixiRecord::Binary(_) => None,
            PixiRecord::Source(record) => Some(record),
        }
    }

    /// Returns a mutable reference to the binary record if it is a binary
    /// record.
    ///
    /// If other `Arc` clones of the record exist, this clones the inner value
    /// first (clone-on-write).
    pub fn as_binary_mut(&mut self) -> Option<&mut RepoDataRecord> {
        match self {
            PixiRecord::Binary(record) => Some(Arc::make_mut(record)),
            PixiRecord::Source(_) => None,
        }
    }

    /// Returns the source record if it is a source record.
    pub fn as_source(&self) -> Option<&SourceRecord> {
        match self {
            PixiRecord::Binary(_) => None,
            PixiRecord::Source(record) => Some(record),
        }
    }
}

impl From<SourceRecord> for PixiRecord {
    fn from(value: SourceRecord) -> Self {
        PixiRecord::Source(Arc::new(value))
    }
}

impl From<Arc<SourceRecord>> for PixiRecord {
    fn from(value: Arc<SourceRecord>) -> Self {
        PixiRecord::Source(value)
    }
}

impl From<RepoDataRecord> for PixiRecord {
    fn from(value: RepoDataRecord) -> Self {
        PixiRecord::Binary(Arc::new(value))
    }
}

impl From<Arc<RepoDataRecord>> for PixiRecord {
    fn from(value: Arc<RepoDataRecord>) -> Self {
        PixiRecord::Binary(value)
    }
}

/// A record that may contain partial source metadata (not yet resolved).
///
/// Lifecycle: lock file read produces `UnresolvedPixiRecord` values. Binary
/// records and immutable source records are already resolved; mutable source
/// records are partial and must be resolved by re-evaluating source metadata
/// before the record can be used for solving or installing.
///
/// Call [`try_into_resolved`](Self::try_into_resolved) to attempt the
/// conversion to a fully-resolved [`PixiRecord`].
#[derive(Debug, Clone)]
pub enum UnresolvedPixiRecord {
    Binary(Arc<RepoDataRecord>),
    Source(Arc<UnresolvedSourceRecord>),
}

/// Identity-based hashing and equality.
///
/// Binary records are identified by their URL; source records by their
/// `(name, manifest_source, identifier_hash)` tuple. The default derive
/// would recurse through `Source(_)`'s `build_packages` / `host_packages`,
/// which is exponential on deeply-shared ROS graphs. Each child's
/// identifier already encapsulates its content, so identity equality is
/// content-equivalent.
impl std::hash::Hash for UnresolvedPixiRecord {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            UnresolvedPixiRecord::Binary(binary) => {
                0u8.hash(state);
                binary.url.as_str().hash(state);
            }
            UnresolvedPixiRecord::Source(source) => {
                1u8.hash(state);
                source.name().as_source().hash(state);
                source.manifest_source.hash(state);
                source.identifier_hash.hash(state);
            }
        }
    }
}

impl PartialEq for UnresolvedPixiRecord {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (UnresolvedPixiRecord::Binary(a), UnresolvedPixiRecord::Binary(b)) => a.url == b.url,
            (UnresolvedPixiRecord::Source(a), UnresolvedPixiRecord::Source(b)) => {
                a.name() == b.name()
                    && a.manifest_source == b.manifest_source
                    && a.identifier_hash == b.identifier_hash
            }
            _ => false,
        }
    }
}

impl Eq for UnresolvedPixiRecord {}

impl UnresolvedPixiRecord {
    /// The name of the package.
    pub fn name(&self) -> &PackageName {
        match self {
            UnresolvedPixiRecord::Binary(record) => &record.package_record.name,
            UnresolvedPixiRecord::Source(record) => record.name(),
        }
    }

    /// Run-time dependencies.
    pub fn depends(&self) -> &[String] {
        match self {
            UnresolvedPixiRecord::Binary(record) => &record.package_record.depends,
            UnresolvedPixiRecord::Source(record) => record.depends(),
        }
    }

    /// Source dependency locations. Empty for binary records.
    pub fn sources(&self) -> &std::collections::BTreeMap<String, pixi_spec::SourceLocationSpec> {
        static EMPTY: std::sync::LazyLock<
            std::collections::BTreeMap<String, pixi_spec::SourceLocationSpec>,
        > = std::sync::LazyLock::new(std::collections::BTreeMap::new);
        match self {
            UnresolvedPixiRecord::Binary(_) => &EMPTY,
            UnresolvedPixiRecord::Source(record) => record.sources(),
        }
    }

    /// Returns a reference to the binary record if it is one.
    pub fn as_binary(&self) -> Option<&RepoDataRecord> {
        match self {
            UnresolvedPixiRecord::Binary(record) => Some(record),
            UnresolvedPixiRecord::Source(_) => None,
        }
    }

    /// Returns a reference to the source record if it is one.
    pub fn as_source(&self) -> Option<&UnresolvedSourceRecord> {
        match self {
            UnresolvedPixiRecord::Binary(_) => None,
            UnresolvedPixiRecord::Source(record) => Some(record),
        }
    }

    /// Returns the full package record if available (binary or full source).
    pub fn package_record(&self) -> Option<&PackageRecord> {
        match self {
            UnresolvedPixiRecord::Binary(record) => Some(&record.package_record),
            UnresolvedPixiRecord::Source(record) => match &record.data {
                SourceRecordData::Full(full) => Some(&full.package_record),
                SourceRecordData::Partial(_) => None,
            },
        }
    }

    /// Returns true if this is a partial source record.
    pub fn is_partial(&self) -> bool {
        matches!(
            self,
            UnresolvedPixiRecord::Source(s) if s.data.is_partial()
        )
    }

    /// Create from lock file `CondaPackageData`.
    pub fn from_conda_package_data(
        data: CondaPackageData,
        workspace_root: &std::path::Path,
        build_packages: Vec<UnresolvedPixiRecord>,
        host_packages: Vec<UnresolvedPixiRecord>,
    ) -> Result<Self, ParseLockFileError> {
        match data {
            CondaPackageData::Binary(value) => {
                let location = value.location.clone();
                let record: RepoDataRecord = (*value).try_into().map_err(|err| match err {
                    ConversionError::Missing(field) => ParseLockFileError::Missing(location, field),
                    ConversionError::LocationToUrlConversionError(err) => {
                        ParseLockFileError::InvalidRecordUrl(location, err)
                    }
                    ConversionError::InvalidBinaryPackageLocation => {
                        ParseLockFileError::InvalidArchiveFilename(location)
                    }
                })?;
                Ok(UnresolvedPixiRecord::Binary(Arc::new(record)))
            }
            CondaPackageData::Source(value) => Ok(UnresolvedPixiRecord::Source(Arc::new(
                UnresolvedSourceRecord::from_conda_source_data(
                    *value,
                    workspace_root,
                    build_packages,
                    host_packages,
                )?,
            ))),
        }
    }

    /// Convert to `CondaPackageData` for lock file write. Source records'
    /// `build_packages` / `host_packages` are registered into the writer's
    /// builder recursively, with the writer's cache deduping shared
    /// subtrees. See [`LockFileWriter`].
    pub fn into_conda_package_data(
        self,
        writer: &mut LockFileWriter<'_>,
        workspace_root: &Path,
    ) -> CondaPackageData {
        match self {
            UnresolvedPixiRecord::Binary(record) => Arc::unwrap_or_clone(record).into(),
            UnresolvedPixiRecord::Source(record) => {
                let mut source = Arc::unwrap_or_clone(record);
                let source_data = register_source_deps(writer, &mut source, workspace_root);
                let mut data = source.into_conda_source_data(workspace_root);
                data.source_data = source_data;
                CondaPackageData::Source(Box::new(data))
            }
        }
    }

    /// Try to convert into a fully resolved [`PixiRecord`].
    ///
    /// Returns `Ok(PixiRecord)` if this is a binary record or a source record
    /// with full metadata. Returns `Err(self)` if this is a partial source
    /// record that still needs metadata resolution (i.e. re-evaluation of
    /// the mutable source).
    pub fn try_into_resolved(self) -> Result<PixiRecord, Self> {
        match self {
            UnresolvedPixiRecord::Binary(record) => Ok(PixiRecord::Binary(record)),
            UnresolvedPixiRecord::Source(source) => {
                if source.data.is_full() {
                    // Downcast SourceRecord<SourceRecordData> -> SourceRecord<FullSourceRecordData>.
                    // This has to reassemble the struct so a fresh Arc allocation is needed.
                    let full = Arc::unwrap_or_clone(source).map_data(|data| match data {
                        SourceRecordData::Full(full) => full,
                        SourceRecordData::Partial(_) => {
                            unreachable!("guarded by is_full() check above")
                        }
                    });
                    Ok(PixiRecord::Source(Arc::new(full)))
                } else {
                    // Partial: return the same Arc untouched.
                    Err(UnresolvedPixiRecord::Source(source))
                }
            }
        }
    }
}

#[derive(Debug, Hash, Eq, PartialEq, Clone)]
struct SourceCacheKey {
    name: PackageName,
    hash: String,
    location: UrlOrPath,
}

/// Memoizes registrations of shared source subtrees so a record reached
/// under multiple parents is descended once. Source records are the only
/// recursive case; binary records are leaves and must keep flowing through
/// `LockFileBuilder::register_conda_package` so its per-URL `merge` can
/// layer run_exports / purls / hashes from later registrations onto an
/// earlier one.
///
/// Keyed on the same identity rattler_lock uses to dedupe source records
/// (`(name, identifier_hash, location)`). Pointer-identity keys would miss
/// because `From<PixiRecord> for UnresolvedPixiRecord` rewraps source Arcs
/// at the dispatcher boundary.
#[derive(Default)]
struct RegistrationCache {
    sources: HashMap<SourceCacheKey, PackageHandle>,
}

/// Pairs a [`LockFileBuilder`] with a [`RegistrationCache`] so they flow
/// through the lock file write path as one argument.
///
/// Construct one at the start of a rebuild and pass it to every
/// `into_conda_package_data` call against the same builder. The cache lives
/// for as long as the writer does, so dedup spans the whole rebuild. Use
/// `writer.builder` directly for non-registration builder calls.
pub struct LockFileWriter<'b> {
    pub builder: &'b mut LockFileBuilder,
    cache: RegistrationCache,
}

impl<'b> LockFileWriter<'b> {
    pub fn new(builder: &'b mut LockFileBuilder) -> Self {
        Self {
            builder,
            cache: RegistrationCache::default(),
        }
    }
}

/// Register a source record's `build_packages` / `host_packages` recursively
/// and build a [`SourceData`] referencing the resulting handles. Drains
/// `source`'s build/host vecs; canonical form lives in the lock file after.
fn register_source_deps<D>(
    writer: &mut LockFileWriter<'_>,
    source: &mut source_record::SourceRecord<D>,
    workspace_root: &Path,
) -> SourceData {
    let build_handles = register_handles(
        writer,
        std::mem::take(&mut source.build_packages),
        workspace_root,
    );
    let host_handles = register_handles(
        writer,
        std::mem::take(&mut source.host_packages),
        workspace_root,
    );
    SourceData {
        build_packages: EnvironmentPackages::from_handles(build_handles)
            .expect("handles just produced by this builder"),
        host_packages: EnvironmentPackages::from_handles(host_handles)
            .expect("handles just produced by this builder"),
    }
}

fn register_handles(
    writer: &mut LockFileWriter<'_>,
    deps: Vec<UnresolvedPixiRecord>,
    workspace_root: &Path,
) -> Vec<PackageHandle> {
    deps.into_iter()
        .map(|dep| register_dep(writer, dep, workspace_root))
        .collect()
}

/// Register a single build/host dependency, hitting the cache first.
fn register_dep(
    writer: &mut LockFileWriter<'_>,
    dep: UnresolvedPixiRecord,
    workspace_root: &Path,
) -> PackageHandle {
    match dep {
        UnresolvedPixiRecord::Binary(record) => {
            // Binaries are leaves: no recursive walk to skip. Always go
            // through `register_conda_package` so its per-URL merge can
            // layer run_exports / purls / hashes onto an earlier
            // registration that lacked them.
            let data: CondaPackageData = Arc::unwrap_or_clone(record).into();
            writer.builder.register_conda_package(data)
        }
        UnresolvedPixiRecord::Source(record) => {
            let key = SourceCacheKey {
                name: record.name().clone(),
                hash: record.identifier_hash.clone(),
                location: record.manifest_source.clone().into(),
            };
            if let Some(handle) = writer.cache.sources.get(&key) {
                return handle.clone();
            }
            let mut source = Arc::unwrap_or_clone(record);
            let source_data = register_source_deps(writer, &mut source, workspace_root);
            let mut data = source.into_conda_source_data(workspace_root);
            data.source_data = source_data;
            let handle = writer
                .builder
                .register_conda_package(CondaPackageData::Source(Box::new(data)));
            writer.cache.sources.insert(key, handle.clone());
            handle
        }
    }
}

impl From<PixiRecord> for UnresolvedPixiRecord {
    fn from(record: PixiRecord) -> Self {
        match record {
            PixiRecord::Binary(r) => UnresolvedPixiRecord::Binary(r),
            PixiRecord::Source(r) => {
                let full = Arc::unwrap_or_clone(r);
                UnresolvedPixiRecord::Source(Arc::new(full.into()))
            }
        }
    }
}

#[derive(Debug, Error)]
pub enum ParseLockFileError {
    #[error("missing field/fields '{1}' for package {0}")]
    Missing(UrlOrPath, String),

    #[error("Invalid archive file name for package {0}")]
    InvalidArchiveFilename(UrlOrPath),

    #[error("invalid url for package {0}")]
    InvalidRecordUrl(UrlOrPath, #[source] file_url::FileURLParseError),

    #[error(transparent)]
    PinnedSourceSpecError(#[from] pinned_source::ParseError),
}

impl Matches<PixiRecord> for NamelessMatchSpec {
    fn matches(&self, record: &PixiRecord) -> bool {
        match record {
            PixiRecord::Binary(record) => self.matches(record.as_ref()),
            PixiRecord::Source(record) => self.matches(record.as_ref()),
        }
    }
}

impl Matches<PixiRecord> for MatchSpec {
    fn matches(&self, record: &PixiRecord) -> bool {
        match record {
            PixiRecord::Binary(record) => self.matches(record.as_ref()),
            PixiRecord::Source(record) => self.matches(record.as_ref()),
        }
    }
}

impl AsRef<PackageRecord> for PixiRecord {
    fn as_ref(&self) -> &PackageRecord {
        match self {
            PixiRecord::Binary(record) => &record.package_record,
            PixiRecord::Source(record) => record.as_ref().as_ref(),
        }
    }
}
