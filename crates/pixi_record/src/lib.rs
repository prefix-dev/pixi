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
    ChannelUrl, MatchSpec, Matches, NamelessMatchSpec, PackageName, PackageRecord, RepoDataRecord,
};
use rattler_lock::{
    CondaPackageData, ConversionError, EnvironmentPackages, LockFileBuilder, PackageHandle,
    SourceData, UrlOrPath, Verbatim,
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

/// A stable, order-independent fingerprint over a set of conda records.
///
/// Used to scope PyPI source-build cache keys per environment, so a wheel
/// built against one set of conda dependencies isn't reused in another that
/// resolves different ones. See <https://github.com/prefix-dev/pixi/issues/6226>.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CondaEnvironmentFingerprint(u64);

impl CondaEnvironmentFingerprint {
    /// Computes the fingerprint over the given conda records.
    pub fn new<'a>(records: impl IntoIterator<Item = &'a PixiRecord>) -> Self {
        use std::hash::{Hash, Hasher};

        use xxhash_rust::xxh3::Xxh3;

        // Names are unique within an environment, so sorting by name makes the
        // result order-independent.
        let mut records: Vec<&PixiRecord> = records.into_iter().collect();
        records.sort_unstable_by(|a, b| {
            a.package_record()
                .name
                .as_normalized()
                .cmp(b.package_record().name.as_normalized())
        });

        let mut hasher = Xxh3::new();
        for record in records {
            let package_record = record.package_record();
            package_record.name.as_normalized().hash(&mut hasher);
            package_record.version.to_string().hash(&mut hasher);
            package_record.build.hash(&mut hasher);
            package_record.subdir.hash(&mut hasher);
            if let Some(sha256) = package_record.sha256.as_ref() {
                sha256.as_slice().hash(&mut hasher);
            }
            // Source packages have no binary hash; use the content-derived
            // identifier and pinned source instead.
            if let Some(source) = record.as_source() {
                source.identifier_hash.hash(&mut hasher);
                source.manifest_source.hash(&mut hasher);
            }
        }
        Self(hasher.finish())
    }
}

impl std::fmt::Display for CondaEnvironmentFingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
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
                // A binary package from a local channel may be recorded with a
                // path relative to the lock file (see pixi#6322). Resolve it
                // against the workspace root so the record carries an absolute
                // `file://` URL, mirroring how source records are resolved.
                let mut value = *value;
                let location = resolve_local_location(value.location.inner(), workspace_root);
                value.location = Verbatim::new(location.clone());
                let record: RepoDataRecord = value.try_into().map_err(|err| match err {
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

/// Resolve a binary package location against the workspace root.
///
/// Local-channel packages may be recorded with a path relative to the lock
/// file (see pixi#6322); turn such a path into an absolute `file://` URL so the
/// record carries a location the rest of pixi can use directly. URLs and
/// absolute paths are returned unchanged.
fn resolve_local_location(location: &UrlOrPath, workspace_root: &Path) -> UrlOrPath {
    let UrlOrPath::Path(path) = location else {
        return location.clone();
    };
    if path.is_absolute() {
        return location.clone();
    }
    let absolute = workspace_root.join(path.as_str());
    url::Url::from_file_path(&absolute).map_or_else(|()| location.clone(), UrlOrPath::Url)
}

/// Render a binary package's location relative to a local channel for the lock.
///
/// Each entry of `relative_channels` is the absolute base URL of a channel that
/// was *declared with a relative path*, carrying that declared spelling as its
/// verbatim `given` (e.g. inner `file:///abs/local-channel/`, given
/// `../local-channel`). A binary package served from such a channel gets a
/// verbatim relative `given` so the committed lock stays portable (pixi#6322),
/// while the resolved absolute URL is preserved as the location's inner value.
/// Packages from remote or absolute channels are returned unchanged.
///
/// This is the write-side counterpart to [`resolve_local_location`].
pub fn relativize_local_channel_location(
    data: CondaPackageData,
    relative_channels: &[Verbatim<ChannelUrl>],
) -> CondaPackageData {
    let CondaPackageData::Binary(mut binary) = data else {
        return data;
    };
    // `From<RepoDataRecord>` normalizes a `file://` URL into an absolute path,
    // so compare in URL space: turn the location back into a `file://` URL.
    // Remote (https) locations also produce a URL here but won't match a local
    // channel's `file://` base, so they pass through untouched.
    let Ok(url) = binary.location.inner().try_into_url() else {
        return CondaPackageData::Binary(binary);
    };
    let url = url.as_str();
    for channel in relative_channels {
        let Some(declared_spelling) = channel.given() else {
            continue;
        };
        let base = channel.inner().as_str().trim_end_matches('/');
        let Some(suffix) = url.strip_prefix(base) else {
            continue;
        };
        let suffix = suffix.trim_start_matches('/');
        let given = format!("{}/{suffix}", declared_spelling.trim_end_matches('/'));
        let inner = binary.location.inner().clone();
        binary.location = Verbatim::new_with_given(inner, given);
        break;
    }
    CondaPackageData::Binary(binary)
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

/// Pairs a [`LockFileBuilder`] with a private registration cache so they
/// flow through the lock file write path as one argument.
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, str::FromStr};

    use rattler_conda_types::{VersionWithSource, package::DistArchiveIdentifier};

    use super::*;

    fn binary_record(name: &str, version: &str, build: &str) -> PixiRecord {
        let mut package_record = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            version.parse::<VersionWithSource>().unwrap(),
            build.to_string(),
        );
        package_record.subdir = "linux-64".into();
        let record = RepoDataRecord {
            package_record,
            identifier: DistArchiveIdentifier::try_from_filename(&format!(
                "{name}-{version}-{build}.conda"
            ))
            .unwrap(),
            url: url::Url::parse("https://example.com/pkg.conda").unwrap(),
            channel: None,
        };
        PixiRecord::Binary(Arc::new(record))
    }

    /// A binary package resolved from a local channel keeps a relative `conda:`
    /// path in the lock when that channel was declared with a relative path.
    /// Write-side counterpart to the read-side resolution; covers pixi#6322.
    #[test]
    fn relativize_local_channel_location_uses_declared_spelling() {
        let channel: ChannelUrl = url::Url::parse("file:///workspace/local-channel/")
            .unwrap()
            .into();
        let relative_channels =
            vec![Verbatim::new_with_given(channel, "../local-channel".to_string())];

        let mut package_record = PackageRecord::new(
            PackageName::from_str("my-dep").unwrap(),
            "0.1.0".parse::<VersionWithSource>().unwrap(),
            "h0".to_string(),
        );
        package_record.subdir = "linux-64".into();
        let data = CondaPackageData::from(RepoDataRecord {
            package_record,
            identifier: DistArchiveIdentifier::try_from_filename("my-dep-0.1.0-h0.conda").unwrap(),
            url: url::Url::parse(
                "file:///workspace/local-channel/linux-64/my-dep-0.1.0-h0.conda",
            )
            .unwrap(),
            channel: Some("file:///workspace/local-channel/".to_string()),
        });

        let data = relativize_local_channel_location(data, &relative_channels);
        let binary = data.as_binary().expect("binary package");
        assert_eq!(
            binary.location.given(),
            Some("../local-channel/linux-64/my-dep-0.1.0-h0.conda"),
            "the lock should store the path relative to the declared channel"
        );
    }

    /// A package from a remote channel is left untouched by relativization.
    #[test]
    fn relativize_leaves_remote_packages_untouched() {
        let channel: ChannelUrl = url::Url::parse("file:///workspace/local-channel/")
            .unwrap()
            .into();
        let relative_channels =
            vec![Verbatim::new_with_given(channel, "../local-channel".to_string())];

        let PixiRecord::Binary(record) = binary_record("foo", "1.0.0", "h0") else {
            unreachable!("binary_record returns a binary record");
        };
        let data = CondaPackageData::from(Arc::unwrap_or_clone(record));
        let data = relativize_local_channel_location(data, &relative_channels);
        assert_eq!(data.as_binary().unwrap().location.given(), None);
    }

    fn source_record(name: &str, identifier_hash: &str, path: &str) -> PixiRecord {
        let mut package_record = PackageRecord::new(
            PackageName::from_str(name).unwrap(),
            "1.0.0".parse::<VersionWithSource>().unwrap(),
            "h0".to_string(),
        );
        package_record.subdir = "linux-64".into();
        let record = SourceRecord {
            data: FullSourceRecordData {
                package_record,
                sources: BTreeMap::new(),
            },
            manifest_source: PinnedSourceSpec::Path(PinnedPathSpec {
                path: typed_path::Utf8TypedPathBuf::from(path),
            }),
            build_source: None,
            variants: BTreeMap::new(),
            identifier_hash: identifier_hash.to_string(),
            build_packages: Vec::new(),
            host_packages: Vec::new(),
        };
        PixiRecord::Source(Arc::new(record))
    }

    fn fingerprint(records: &[PixiRecord]) -> String {
        CondaEnvironmentFingerprint::new(records).to_string()
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let gdal = binary_record("libgdal", "3.10.3", "h0");
        let python = binary_record("python", "3.12.0", "h1");
        assert_eq!(
            fingerprint(&[gdal.clone(), python.clone()]),
            fingerprint(&[python, gdal]),
        );
    }

    #[test]
    fn fingerprint_changes_with_conda_dependency_version() {
        // The motivating case from issue #6226.
        let gdal_310 = binary_record("libgdal", "3.10.3", "h0");
        let gdal_313 = binary_record("libgdal", "3.13.0", "h0");
        assert_ne!(fingerprint(&[gdal_310]), fingerprint(&[gdal_313]));
    }

    #[test]
    fn fingerprint_changes_with_conda_dependency_build() {
        let gdal_h0 = binary_record("libgdal", "3.10.3", "h0");
        let gdal_h1 = binary_record("libgdal", "3.10.3", "h1");
        assert_ne!(fingerprint(&[gdal_h0]), fingerprint(&[gdal_h1]));
    }

    #[test]
    fn fingerprint_changes_with_source_identifier() {
        let a = source_record("my-pkg", "aaaaaaaa", "./my-pkg");
        let b = source_record("my-pkg", "bbbbbbbb", "./my-pkg");
        assert_ne!(fingerprint(&[a]), fingerprint(&[b]));
    }

    #[test]
    fn fingerprint_changes_with_source_location() {
        let a = source_record("my-pkg", "aaaaaaaa", "./my-pkg");
        let b = source_record("my-pkg", "aaaaaaaa", "./other");
        assert_ne!(fingerprint(&[a]), fingerprint(&[b]));
    }
}
