use std::{
    collections::{BTreeMap, btree_map},
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

use pixi_path::normalize_std;
use rattler_conda_types::{PackageName, PrefixRecord, prefix_record::PathType};
use rattler_digest::{Sha256, Sha256Hash, compute_bytes_digest, compute_file_digest};
use uv_distribution_types::{CachedDist, Name};
use uv_install_wheel::RecordEntry;
use uv_python::PythonEnvironment;

use ahash::{AHashMap, AHashSet};

use super::install_wheel::{LibKind, get_wheel_info};

const MAX_CLOBBER_PATHS_PER_PACKAGE: usize = 5;

#[derive(Default, Debug)]
pub(crate) struct ClobberReport(BTreeMap<(String, String), Vec<CondaPrefixPath>>);

impl ClobberReport {
    fn entry(
        &mut self,
        key: (String, String),
    ) -> btree_map::Entry<'_, (String, String), Vec<CondaPrefixPath>> {
        self.0.entry(key)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn keys(&self) -> btree_map::Keys<'_, (String, String), Vec<CondaPrefixPath>> {
        self.0.keys()
    }
}

impl fmt::Display for ClobberReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "PyPI package files will overwrite files installed by conda packages:"
        )?;

        for ((pypi_package, conda_package), paths) in &self.0 {
            writeln!(
                f,
                "  - PyPI package '{pypi_package}' overwrites conda package '{conda_package}':"
            )?;

            for path in paths.iter().take(MAX_CLOBBER_PATHS_PER_PACKAGE) {
                writeln!(f, "    - {}", path.as_path().display())?;
            }

            let remaining = paths.len().saturating_sub(MAX_CLOBBER_PATHS_PER_PACKAGE);
            if remaining > 0 {
                writeln!(f, "    - ... {remaining} other files")?;
            }
        }

        Ok(())
    }
}

#[derive(Default, Debug)]
pub(crate) struct PypiCondaClobberRegistry {
    /// A registry of the paths of the installed conda paths and the package names
    paths_registry: AHashMap<CondaPrefixPath, CondaPathOwnership>,
    /// Conda-owned directories that uv must not remove while pruning parents.
    protected_directories: AHashSet<CondaPrefixPath>,
    /// Conda paths grouped by each ancestor directory that contains them.
    protected_directory_descendants: AHashMap<CondaPrefixPath, AHashSet<CondaPrefixPath>>,
    /// Pycache paths grouped by the parent directory whose cleanup uv would visit.
    protected_pycache_paths: AHashMap<PathBuf, AHashSet<CondaPrefixPath>>,
    /// Candidate paths indexed by a case-folded hash. Canonical identity is
    /// still checked before a candidate is used.
    case_folded_paths: AHashMap<u64, AHashSet<CondaPrefixPath>>,
}

#[derive(Debug)]
struct CondaPathOwnership {
    package_name: PackageName,
    path_type: PathType,
    expected_sha256: Option<Sha256Hash>,
}

#[derive(Default, Debug)]
pub(crate) struct CondaRecordPathProtection {
    pub(crate) owned: AHashSet<String>,
    /// RECORD paths withheld from uv because raw traversal is unsafe or
    /// recursive removal would delete a conda-owned path.
    pub(crate) unsafe_to_remove: AHashSet<String>,
    pub(crate) cleanup_sensitive: AHashSet<String>,
    pub(crate) protected_pycache_paths: AHashMap<PathBuf, AHashSet<PathBuf>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CondaPathState {
    Untracked,
    Owned,
    Clobbered(PackageName),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WheelDataScheme {
    Purelib,
    Platlib,
    Headers,
    Scripts,
    Data,
}

fn parse_wheel_data_path(record_path: &Path) -> Option<(WheelDataScheme, &Path)> {
    let mut components = record_path.components();
    let data_dir = components.next()?;
    let scheme = components.next()?;

    if Path::new(data_dir.as_os_str()).extension() != Some("data".as_ref()) {
        return None;
    }

    let scheme = match scheme.as_os_str().to_str()? {
        "purelib" => WheelDataScheme::Purelib,
        "platlib" => WheelDataScheme::Platlib,
        "headers" => WheelDataScheme::Headers,
        "scripts" => WheelDataScheme::Scripts,
        "data" => WheelDataScheme::Data,
        _ => return None,
    };

    Some((scheme, components.as_path()))
}

/// The destinations wheel files are installed to, in prefix-relative form.
///
/// Derived from the same layout that uv's installer writes with
/// ([`uv_python::Interpreter::layout`]), so the prediction cannot drift from
/// the actual writes. The absolute layout paths are relative-ized against
/// the interpreter's own `sys_prefix`: both values come from a single
/// interpreter probe and therefore cannot disagree about path spelling
/// (e.g. resolved symlinks) the way two independently-derived paths could.
struct WheelInstallPaths {
    purelib: PathBuf,
    platlib: PathBuf,
    headers: PathBuf,
    scripts: PathBuf,
    data: PathBuf,
}

impl WheelInstallPaths {
    /// Returns `None` when the interpreter's install scheme does not live
    /// inside its `sys_prefix`, which cannot happen for a conda environment.
    fn from_environment(venv: &PythonEnvironment) -> Option<Self> {
        let interpreter = venv.interpreter();
        let sys_prefix = interpreter.sys_prefix();
        let scheme = interpreter.layout().scheme;
        let rel = |path: PathBuf| -> Option<PathBuf> {
            path.strip_prefix(sys_prefix).ok().map(Path::to_path_buf)
        };
        Some(Self {
            purelib: rel(scheme.purelib)?,
            platlib: rel(scheme.platlib)?,
            headers: rel(scheme.include)?,
            scripts: rel(scheme.scripts)?,
            data: rel(scheme.data)?,
        })
    }
}

fn wheel_record_install_path(
    install_paths: &WheelInstallPaths,
    kind: LibKind,
    record_path: impl AsRef<Path>,
) -> PathBuf {
    let record_path = record_path.as_ref();

    if let Some((scheme, relative_path)) = parse_wheel_data_path(record_path) {
        // PEP 427 "spreads" `{distribution}-{version}.data/<scheme>/*`
        // into the corresponding installation scheme destination.
        return match scheme {
            WheelDataScheme::Purelib => install_paths.purelib.join(relative_path),
            WheelDataScheme::Platlib => install_paths.platlib.join(relative_path),
            WheelDataScheme::Headers => install_paths.headers.join(relative_path),
            WheelDataScheme::Scripts => install_paths.scripts.join(relative_path),
            WheelDataScheme::Data => install_paths.data.join(relative_path),
        };
    }

    match kind {
        LibKind::Plat => install_paths.platlib.join(record_path),
        // `Unknown` never reaches this point: `get_wheel_info` filters it out.
        LibKind::Pure | LibKind::Unknown => install_paths.purelib.join(record_path),
    }
}

/// A normalized path in the prefix-relative form conda's `paths.json` uses,
/// e.g. `lib/python3.12/site-packages/boltons/__init__.py`.
///
/// Conda-installed paths and wheel RECORD entries can only be compared in
/// this form; the constructors are the only way to obtain a value, so the
/// convention cannot be mixed up with absolute or differently-rooted paths.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CondaPrefixPath(PathBuf);

impl CondaPrefixPath {
    fn from_prefix_relative(path: PathBuf) -> Option<Self> {
        match path.components().next() {
            Some(std::path::Component::Normal(_)) => Some(Self(path)),
            _ => None,
        }
    }

    /// From a conda `PrefixRecord` path, which should be prefix-relative by
    /// definition. Returns `None` for a malformed (non-relative) entry: such
    /// a key could never match a wheel-side path anyway, and the clobber
    /// check is best-effort.
    fn from_conda_record(path: PathBuf) -> Option<Self> {
        if path.is_relative() {
            Some(Self(path))
        } else {
            tracing::debug!(
                "ignoring non-relative conda paths.json entry `{}` in the clobber registry",
                path.display()
            );
            None
        }
    }

    /// Convert a wheel RECORD entry to the prefix-relative form, or `None`
    /// if the file lands outside the prefix.
    fn from_wheel_record(
        install_paths: &WheelInstallPaths,
        kind: LibKind,
        record_path: impl AsRef<Path>,
    ) -> Option<Self> {
        let path = normalize_std(&wheel_record_install_path(install_paths, kind, record_path));
        // All install destinations are prefix-relative, so the joined path is
        // too — unless the RECORD entry escapes the prefix. A normalized path
        // escapes when it does not start with a normal component: a leading
        // `..` is a relative escape, and a leading root or drive prefix means
        // the RECORD entry was absolute-ish and replaced the base on `join`
        // (note that on Windows `is_absolute()` would miss root-relative
        // paths like `\abs\evil`, hence the component check).
        Self::from_prefix_relative(path)
    }

    /// Convert an installed wheel RECORD entry to prefix-relative form.
    /// Installed RECORD paths are resolved relative to site-packages by uv,
    /// including entries such as `../../../bin/tool`.
    fn from_installed_wheel_record(
        prefix: &Path,
        site_packages: &Path,
        record_path: impl AsRef<Path>,
    ) -> Option<Self> {
        let installed_path = normalize_std(&site_packages.join(record_path.as_ref()));
        let relative_path = installed_path.strip_prefix(prefix).ok()?.to_path_buf();
        Self::from_prefix_relative(relative_path)
    }

    fn as_path(&self) -> &Path {
        &self.0
    }
}

fn has_symlink_ancestor(prefix: &Path, path: &CondaPrefixPath) -> io::Result<bool> {
    let mut ancestor = prefix.to_path_buf();
    if let Some(parent) = path.as_path().parent() {
        for component in parent.components() {
            ancestor.push(component);
            match fs_err::symlink_metadata(&ancestor) {
                Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => return Ok(true),
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(err) => return Err(err),
            }
        }
    }
    Ok(false)
}

fn metadata_is_symlink_or_reparse(metadata: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }

    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn installed_record_path_is_unsafe(
    prefix: &Path,
    site_packages: &Path,
    record_path: &Path,
) -> io::Result<bool> {
    if matches!(
        record_path.components().next_back(),
        None | Some(std::path::Component::Prefix(_))
            | Some(std::path::Component::RootDir)
            | Some(std::path::Component::CurDir)
            | Some(std::path::Component::ParentDir)
    ) {
        return Ok(true);
    }

    let first_component = record_path.components().next();
    // Walk the raw path from the trusted prefix before lexical normalization.
    // Otherwise `..` could erase a symlink or reparse-point ancestor.
    let relative_path = if matches!(
        first_component,
        Some(std::path::Component::Prefix(_) | std::path::Component::RootDir)
    ) {
        let Ok(relative_path) = record_path.strip_prefix(prefix) else {
            return Ok(true);
        };
        relative_path.to_path_buf()
    } else {
        let Ok(site_packages_relative) = site_packages.strip_prefix(prefix) else {
            return Ok(true);
        };
        site_packages_relative.join(record_path)
    };

    let mut ancestor = prefix.to_path_buf();
    let mut components = relative_path.components().peekable();
    while let Some(component) = components.next() {
        if components.peek().is_none() {
            break;
        }
        ancestor.push(component.as_os_str());
        match fs_err::symlink_metadata(&ancestor) {
            Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => return Ok(true),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(true),
            Err(err) => return Err(err),
        }
    }

    let normalized_path = normalize_std(&site_packages.join(record_path));
    Ok(filesystem_relative_path(prefix, &normalized_path)?.is_none())
}

pub(crate) fn filesystem_relative_path(base: &Path, path: &Path) -> io::Result<Option<PathBuf>> {
    let base = normalize_std(base);
    let path = normalize_std(path);
    if let Ok(relative_path) = path.strip_prefix(&base) {
        return Ok(Some(relative_path.to_path_buf()));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let base_metadata = match fs_err::metadata(&base) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        for ancestor in path.ancestors() {
            let metadata = match fs_err::metadata(ancestor) {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            if (metadata.dev(), metadata.ino()) == (base_metadata.dev(), base_metadata.ino()) {
                return Ok(Some(
                    path.strip_prefix(ancestor)
                        .expect("ancestor comes from Path::ancestors")
                        .to_path_buf(),
                ));
            }
        }
    }

    #[cfg(not(unix))]
    {
        let canonical_base = match fs_err::canonicalize(&base) {
            Ok(path) => path,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        for ancestor in path.ancestors() {
            let canonical_ancestor = match fs_err::canonicalize(ancestor) {
                Ok(path) => path,
                Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                Err(err) => return Err(err),
            };
            if canonical_ancestor == canonical_base {
                return Ok(Some(
                    path.strip_prefix(ancestor)
                        .expect("ancestor comes from Path::ancestors")
                        .to_path_buf(),
                ));
            }
        }
    }

    Ok(None)
}

/// Return the prefix-relative path only when recursive removal cannot traverse
/// a symbolic link or Windows reparse point below the trusted prefix root.
pub(crate) fn recursive_removal_relative_path(prefix: &Path, path: &Path) -> io::Result<PathBuf> {
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::CurDir | std::path::Component::ParentDir
        )
    }) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to recursively remove {} with a non-normal path component",
                path.display()
            ),
        ));
    }

    let Some(relative_path) = filesystem_relative_path(prefix, path)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "path {} is not inside prefix {}",
                path.display(),
                prefix.display()
            ),
        ));
    };
    if relative_path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to recursively remove prefix root {}",
                prefix.display()
            ),
        ));
    }

    let mut installed_path = normalize_std(prefix);
    for component in relative_path.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path {} is not prefix-relative", path.display()),
            ));
        };
        installed_path.push(component);
        match fs_err::symlink_metadata(&installed_path) {
            Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "refusing to recursively remove {} through symbolic link or reparse point {}",
                        path.display(),
                        installed_path.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => break,
            Err(err) => return Err(err),
        }
    }

    Ok(relative_path)
}

fn resolve_prefix_paths(prefix: &Path, site_packages: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let prefix = normalize_std(prefix);
    let site_packages = normalize_std(site_packages);
    match filesystem_relative_path(&prefix, &site_packages)? {
        Some(relative_path) => Ok((prefix.clone(), prefix.join(relative_path))),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "site-packages {} is not inside prefix {}",
                site_packages.display(),
                prefix.display()
            ),
        )),
    }
}

fn current_path_is_conda_owned(
    prefix: &Path,
    path: &CondaPrefixPath,
    ownership: &CondaPathOwnership,
) -> io::Result<bool> {
    if has_symlink_ancestor(prefix, path)? {
        return Ok(false);
    }
    let installed_path = prefix.join(path.as_path());
    let metadata = match fs_err::symlink_metadata(&installed_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    if ownership.path_type == PathType::Directory {
        return Ok(metadata.file_type().is_dir());
    }

    if ownership.path_type == PathType::SoftLink {
        if metadata.file_type().is_symlink() {
            let target = fs_err::read_link(&installed_path)?;
            let actual_sha256 = compute_bytes_digest::<Sha256>(target.to_string_lossy().as_bytes());
            return Ok(ownership.expected_sha256.as_ref() == Some(&actual_sha256));
        }
        if !metadata.file_type().is_file() {
            return Ok(false);
        }
        let actual_sha256 = compute_file_digest::<Sha256>(&installed_path)?;
        return Ok(ownership.expected_sha256.as_ref() == Some(&actual_sha256));
    }

    if !metadata.file_type().is_file() {
        return Ok(false);
    }

    let actual_sha256 = match compute_file_digest::<Sha256>(&installed_path) {
        Ok(digest) => digest,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    Ok(ownership.expected_sha256.as_ref() == Some(&actual_sha256))
}

fn case_folded_path_hash(path: &CondaPrefixPath) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.as_path()
        .to_string_lossy()
        .split(['/', '\\'])
        .filter(|component| !component.is_empty() && *component != ".")
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .hash(&mut hasher);
    hasher.finish()
}

fn case_folded_file_name(name: &std::ffi::OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}

fn symlink_directory_entries_match(left: &Path, right: &Path) -> io::Result<bool> {
    let Some(left_parent) = left.parent() else {
        return Ok(false);
    };
    let Some(right_parent) = right.parent() else {
        return Ok(false);
    };
    if fs_err::canonicalize(left_parent)? != fs_err::canonicalize(right_parent)? {
        return Ok(false);
    }

    let Some(left_name) = left.file_name() else {
        return Ok(false);
    };
    let Some(right_name) = right.file_name() else {
        return Ok(false);
    };
    let folded_name = case_folded_file_name(left_name);
    if folded_name != case_folded_file_name(right_name) {
        return Ok(false);
    }

    let mut matching_entries = 0;
    for entry in fs_err::read_dir(left_parent)? {
        if case_folded_file_name(&entry?.file_name()) == folded_name {
            matching_entries += 1;
            if matching_entries > 1 {
                return Ok(false);
            }
        }
    }
    Ok(matching_entries == 1)
}

#[cfg(unix)]
fn unix_directory_entries_match(
    left: &Path,
    right: &Path,
    left_metadata: &std::fs::Metadata,
    right_metadata: &std::fs::Metadata,
) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    if (left_metadata.dev(), left_metadata.ino()) != (right_metadata.dev(), right_metadata.ino()) {
        return Ok(false);
    }

    let Some(left_parent) = left.parent() else {
        return Ok(false);
    };
    let Some(right_parent) = right.parent() else {
        return Ok(false);
    };
    let left_parent_metadata = fs_err::symlink_metadata(left_parent)?;
    let right_parent_metadata = fs_err::symlink_metadata(right_parent)?;
    if (left_parent_metadata.dev(), left_parent_metadata.ino())
        != (right_parent_metadata.dev(), right_parent_metadata.ino())
    {
        return Ok(false);
    }

    let mut matching_entries = 0;
    for entry in fs_err::read_dir(left_parent)? {
        let metadata = fs_err::symlink_metadata(entry?.path())?;
        if (metadata.dev(), metadata.ino()) == (left_metadata.dev(), left_metadata.ino()) {
            matching_entries += 1;
            if matching_entries > 1 {
                return Ok(false);
            }
        }
    }
    Ok(matching_entries == 1)
}

fn canonical_paths_match(
    prefix: &Path,
    left: &CondaPrefixPath,
    right: &CondaPrefixPath,
) -> io::Result<bool> {
    if has_symlink_ancestor(prefix, left)? || has_symlink_ancestor(prefix, right)? {
        return Ok(false);
    }

    let left = prefix.join(left.as_path());
    let right = prefix.join(right.as_path());
    let left_metadata = match fs_err::symlink_metadata(&left) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let right_metadata = match fs_err::symlink_metadata(&right) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    let left_is_symlink = metadata_is_symlink_or_reparse(&left_metadata);
    let right_is_symlink = metadata_is_symlink_or_reparse(&right_metadata);
    if left_is_symlink != right_is_symlink {
        return Ok(false);
    }
    #[cfg(unix)]
    if unix_directory_entries_match(&left, &right, &left_metadata, &right_metadata)? {
        return Ok(true);
    }
    if left_is_symlink {
        return symlink_directory_entries_match(&left, &right);
    }

    Ok(fs_err::canonicalize(left)? == fs_err::canonicalize(right)?)
}

impl PypiCondaClobberRegistry {
    /// Register the paths of the installed conda packages
    /// to later check if they are going to be clobbered by the installation of the wheels
    pub(crate) fn with_conda_packages(conda_packages: &[PrefixRecord]) -> Self {
        let mut registry = AHashMap::with_capacity(conda_packages.len() * 50);
        let mut protected_directories = AHashSet::new();
        let mut protected_directory_descendants =
            AHashMap::<CondaPrefixPath, AHashSet<CondaPrefixPath>>::new();
        let mut protected_pycache_paths = AHashMap::<PathBuf, AHashSet<CondaPrefixPath>>::new();
        let mut case_folded_paths = AHashMap::<u64, AHashSet<CondaPrefixPath>>::new();
        for record in conda_packages {
            for entry in &record.paths_data.paths {
                let Some(path) = CondaPrefixPath::from_conda_record(entry.relative_path.clone())
                else {
                    continue;
                };
                let mut parent = PathBuf::new();
                for component in path.as_path().components() {
                    if component.as_os_str() == "__pycache__" {
                        protected_pycache_paths
                            .entry(parent)
                            .or_default()
                            .insert(path.clone());
                        break;
                    }
                    parent.push(component);
                }
                if entry.path_type == PathType::Directory {
                    protected_directories.insert(path.clone());
                }
                for ancestor in path
                    .as_path()
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                    .filter(|ancestor| !ancestor.as_os_str().is_empty())
                {
                    let ancestor = CondaPrefixPath(ancestor.to_path_buf());
                    protected_directory_descendants
                        .entry(ancestor.clone())
                        .or_default()
                        .insert(path.clone());
                    case_folded_paths
                        .entry(case_folded_path_hash(&ancestor))
                        .or_default()
                        .insert(ancestor);
                }
                case_folded_paths
                    .entry(case_folded_path_hash(&path))
                    .or_default()
                    .insert(path.clone());
                registry.insert(
                    path,
                    CondaPathOwnership {
                        package_name: record.repodata_record.package_record.name.clone(),
                        path_type: entry.path_type,
                        expected_sha256: entry.sha256_in_prefix.or(entry.sha256),
                    },
                );
            }
        }
        Self {
            paths_registry: registry,
            protected_directories,
            protected_directory_descendants,
            protected_pycache_paths,
            case_folded_paths,
        }
    }

    fn matching_indexed_path(
        &self,
        prefix: &Path,
        path: &CondaPrefixPath,
        mut is_match: impl FnMut(&CondaPrefixPath) -> bool,
    ) -> io::Result<Option<CondaPrefixPath>> {
        if is_match(path) {
            return Ok(Some(path.clone()));
        }

        if let Some(candidates) = self.case_folded_paths.get(&case_folded_path_hash(path)) {
            for candidate in candidates {
                if is_match(candidate) && canonical_paths_match(prefix, path, candidate)? {
                    return Ok(Some(candidate.clone()));
                }
            }
        }

        // Case folding does not make differently normalized Unicode strings
        // equal, and Windows short names do not resemble their long names.
        // Use filesystem identity only for these uncommon aliases.
        let needs_full_identity_scan = !path.as_path().to_string_lossy().is_ascii()
            || cfg!(windows)
                && path
                    .as_path()
                    .components()
                    .any(|component| component.as_os_str().to_string_lossy().contains('~'));
        if needs_full_identity_scan {
            for candidate in self.case_folded_paths.values().flatten() {
                if is_match(candidate) && canonical_paths_match(prefix, path, candidate)? {
                    return Ok(Some(candidate.clone()));
                }
            }
        }
        Ok(None)
    }

    fn indexed_path_for_installed_path(
        &self,
        prefix: &Path,
        installed_path: &Path,
    ) -> io::Result<Option<CondaPrefixPath>> {
        let Some(relative_path) = filesystem_relative_path(prefix, installed_path)? else {
            return Ok(None);
        };
        let Some(path) = CondaPrefixPath::from_prefix_relative(relative_path) else {
            return Ok(None);
        };
        self.matching_indexed_path(prefix, &path, |candidate| {
            self.paths_registry.contains_key(candidate)
        })
    }

    pub(crate) fn installed_path_state(
        &self,
        prefix: &Path,
        installed_path: &Path,
    ) -> io::Result<CondaPathState> {
        let Some(path) = self.indexed_path_for_installed_path(prefix, installed_path)? else {
            return Ok(CondaPathState::Untracked);
        };
        let ownership = &self.paths_registry[&path];
        if current_path_is_conda_owned(prefix, &path, ownership)? {
            Ok(CondaPathState::Owned)
        } else {
            Ok(CondaPathState::Clobbered(ownership.package_name.clone()))
        }
    }

    fn add_clobbered_package(
        &self,
        prefix: &Path,
        path: &CondaPrefixPath,
        packages: &mut AHashSet<PackageName>,
    ) -> io::Result<()> {
        let Some(ownership) = self.paths_registry.get(path) else {
            return Ok(());
        };
        if !current_path_is_conda_owned(prefix, path, ownership)? {
            packages.insert(ownership.package_name.clone());
        }
        Ok(())
    }

    /// Return Conda packages that must be relinked before removing these wheel
    /// RECORD paths. Relinking first lets the guarded uninstall recognize the
    /// restored Conda contents and preserve them.
    pub(crate) fn packages_requiring_reinstall_for_records<'record>(
        &self,
        prefix: &Path,
        site_packages: &Path,
        records: impl IntoIterator<Item = &'record RecordEntry>,
    ) -> io::Result<AHashSet<PackageName>> {
        let (prefix, site_packages) = resolve_prefix_paths(prefix, site_packages)?;
        let prefix = prefix.as_path();
        let site_packages = site_packages.as_path();
        let mut packages = AHashSet::new();

        for record in records {
            let record_path = Path::new(&record.path);
            if installed_record_path_is_unsafe(prefix, site_packages, record_path)? {
                continue;
            }
            let Some(path) =
                CondaPrefixPath::from_installed_wheel_record(prefix, site_packages, record_path)
            else {
                continue;
            };

            let direct_candidate = self.matching_indexed_path(prefix, &path, |candidate| {
                self.paths_registry.contains_key(candidate)
            })?;
            if let Some(candidate) = &direct_candidate {
                self.add_clobbered_package(prefix, candidate, &mut packages)?;
            }

            let (is_directory, is_missing) =
                match fs_err::symlink_metadata(prefix.join(path.as_path())) {
                    Ok(metadata) => (metadata.file_type().is_dir(), false),
                    Err(err) if err.kind() == io::ErrorKind::NotFound => (false, true),
                    Err(err) => return Err(err),
                };

            // A missing leaf has no filesystem identity of its own. Resolve
            // its nearest existing parent and conservatively repair missing or
            // clobbered Conda descendants before uninstalling the RECORD.
            if direct_candidate.is_none() && is_missing {
                for ancestor in path
                    .as_path()
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                {
                    let ancestor = CondaPrefixPath(ancestor.to_path_buf());
                    let Some(parent) =
                        self.matching_indexed_path(prefix, &ancestor, |candidate| {
                            self.protected_directory_descendants.contains_key(candidate)
                        })?
                    else {
                        continue;
                    };
                    for descendant in &self.protected_directory_descendants[&parent] {
                        self.add_clobbered_package(prefix, descendant, &mut packages)?;
                    }
                    break;
                }
            }

            if is_directory {
                if let Some(directory) = self.matching_indexed_path(prefix, &path, |candidate| {
                    self.protected_directory_descendants.contains_key(candidate)
                })? {
                    for descendant in &self.protected_directory_descendants[&directory] {
                        self.add_clobbered_package(prefix, descendant, &mut packages)?;
                    }
                }
                continue;
            }

            for ancestor in path
                .as_path()
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
            {
                let ancestor = CondaPrefixPath(ancestor.to_path_buf());
                let Some(parent) = self.matching_indexed_path(prefix, &ancestor, |candidate| {
                    self.protected_pycache_paths
                        .contains_key(candidate.as_path())
                })?
                else {
                    continue;
                };
                for conda_path in &self.protected_pycache_paths[parent.as_path()] {
                    self.add_clobbered_package(prefix, conda_path, &mut packages)?;
                }
            }
        }

        Ok(packages)
    }

    pub(crate) fn packages_requiring_reinstall_for_tree(
        &self,
        prefix: &Path,
        root: &Path,
    ) -> io::Result<AHashSet<PackageName>> {
        fn visit(
            registry: &PypiCondaClobberRegistry,
            prefix: &Path,
            path: &Path,
            packages: &mut AHashSet<PackageName>,
        ) -> io::Result<()> {
            let metadata = match fs_err::symlink_metadata(path) {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(err) => return Err(err),
            };
            if metadata_is_symlink_or_reparse(&metadata) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "refusing to recursively remove symbolic link or reparse point {}",
                        path.display()
                    ),
                ));
            }
            if let CondaPathState::Clobbered(package) =
                registry.installed_path_state(prefix, path)?
            {
                packages.insert(package);
            }
            if metadata.file_type().is_dir() {
                for entry in fs_err::read_dir(path)? {
                    visit(registry, prefix, &entry?.path(), packages)?;
                }
            }
            Ok(())
        }

        let relative_root = recursive_removal_relative_path(prefix, root)?;
        let root_path = CondaPrefixPath::from_prefix_relative(relative_root).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("path {} is not prefix-relative", root.display()),
            )
        })?;
        let mut packages = AHashSet::new();

        if let Some(candidate) = self.matching_indexed_path(prefix, &root_path, |candidate| {
            self.paths_registry.contains_key(candidate)
        })? {
            self.add_clobbered_package(prefix, &candidate, &mut packages)?;
        }
        if let Some(directory) = self.matching_indexed_path(prefix, &root_path, |candidate| {
            self.protected_directory_descendants.contains_key(candidate)
        })? {
            for descendant in &self.protected_directory_descendants[&directory] {
                self.add_clobbered_package(prefix, descendant, &mut packages)?;
            }
        }
        visit(self, prefix, root, &mut packages)?;
        Ok(packages)
    }

    fn directory_contains_conda_owned_descendant(
        &self,
        prefix: &Path,
        path: &CondaPrefixPath,
    ) -> io::Result<bool> {
        let Some(directory) = self.matching_indexed_path(prefix, path, |candidate| {
            self.protected_directory_descendants.contains_key(candidate)
        })?
        else {
            return Ok(false);
        };

        let metadata = match fs_err::symlink_metadata(prefix.join(directory.as_path())) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err),
        };
        if !metadata.file_type().is_dir() {
            return Ok(false);
        }

        for descendant in &self.protected_directory_descendants[&directory] {
            let Some(ownership) = self.paths_registry.get(descendant) else {
                continue;
            };
            if current_path_is_conda_owned(prefix, descendant, ownership)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn current_directory_is_conda_owned(
        &self,
        prefix: &Path,
        directory: &Path,
    ) -> io::Result<bool> {
        let Some(relative_path) = filesystem_relative_path(prefix, directory)? else {
            return Ok(false);
        };
        let Some(path) = CondaPrefixPath::from_prefix_relative(relative_path) else {
            return Ok(false);
        };

        let Some(candidate) = self.matching_indexed_path(prefix, &path, |candidate| {
            self.protected_directories.contains(candidate)
        })?
        else {
            return Ok(false);
        };
        let Some(ownership) = self.paths_registry.get(&candidate) else {
            return Ok(false);
        };
        current_path_is_conda_owned(prefix, &candidate, ownership)
    }

    fn protect_pycache_paths(
        &self,
        prefix: &Path,
        parent: &Path,
        protection: &mut CondaRecordPathProtection,
    ) -> io::Result<()> {
        if protection.protected_pycache_paths.contains_key(parent) {
            return Ok(());
        }

        let mut protected_paths = AHashSet::new();
        if let Some(conda_paths) = self.protected_pycache_paths.get(parent) {
            for conda_path in conda_paths {
                let Some(ownership) = self.paths_registry.get(conda_path) else {
                    continue;
                };
                if current_path_is_conda_owned(prefix, conda_path, ownership)? {
                    protected_paths.insert(prefix.join(conda_path.as_path()));
                }
            }
        }
        protection
            .protected_pycache_paths
            .insert(parent.to_path_buf(), protected_paths);
        Ok(())
    }

    /// Return the wheel RECORD entries whose installed paths are owned by a
    /// currently installed conda package.
    pub(crate) fn conda_owned_record_paths<'record>(
        &self,
        prefix: &Path,
        site_packages: &Path,
        records: impl IntoIterator<Item = &'record RecordEntry>,
    ) -> io::Result<CondaRecordPathProtection> {
        let (prefix, site_packages) = resolve_prefix_paths(prefix, site_packages)?;
        let prefix = prefix.as_path();
        let site_packages = site_packages.as_path();

        let mut protection = CondaRecordPathProtection::default();
        for record in records {
            let record_path = record.path.as_str();
            if installed_record_path_is_unsafe(prefix, site_packages, Path::new(record_path))? {
                protection.unsafe_to_remove.insert(record_path.to_owned());
                continue;
            }
            let Some(mut path) =
                CondaPrefixPath::from_installed_wheel_record(prefix, site_packages, record_path)
            else {
                continue;
            };

            if has_symlink_ancestor(prefix, &path)? {
                protection.unsafe_to_remove.insert(record_path.to_owned());
                continue;
            }

            if !self.paths_registry.contains_key(&path)
                && let Some(candidate) = self.matching_indexed_path(prefix, &path, |candidate| {
                    self.paths_registry.contains_key(candidate)
                })?
            {
                path = candidate;
            }

            let uv_will_visit_parent = match fs_err::symlink_metadata(prefix.join(path.as_path())) {
                Ok(metadata) => !metadata.file_type().is_dir(),
                Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                Err(err) => return Err(err),
            };
            let mut cleanup_parent_candidates = Vec::new();
            if uv_will_visit_parent {
                for ancestor in path
                    .as_path()
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                {
                    let ancestor = CondaPrefixPath(ancestor.to_path_buf());
                    if let Some(parent) =
                        self.matching_indexed_path(prefix, &ancestor, |candidate| {
                            self.protected_pycache_paths
                                .contains_key(candidate.as_path())
                        })?
                    {
                        cleanup_parent_candidates.push(parent.0);
                    }
                }
            }
            let mut cleanup_parents = Vec::new();
            for parent in cleanup_parent_candidates {
                self.protect_pycache_paths(prefix, &parent, &mut protection)?;
                if protection
                    .protected_pycache_paths
                    .get(&parent)
                    .is_some_and(|paths| !paths.is_empty())
                {
                    cleanup_parents.push(parent);
                }
            }

            let owned = if let Some(ownership) = self.paths_registry.get(&path) {
                current_path_is_conda_owned(prefix, &path, ownership)?
            } else {
                false
            };
            if owned {
                protection.owned.insert(record_path.to_owned());
            } else {
                if self.directory_contains_conda_owned_descendant(prefix, &path)? {
                    // uv recursively removes directory entries from RECORD.
                    // Leave this entry alone when that would remove a conda path.
                    protection.unsafe_to_remove.insert(record_path.to_owned());
                    continue;
                }

                let mut protected_directory = false;
                for ancestor in path.as_path().ancestors() {
                    let ancestor = CondaPrefixPath(ancestor.to_path_buf());
                    if let Some(candidate) =
                        self.matching_indexed_path(prefix, &ancestor, |candidate| {
                            self.protected_directories.contains(candidate)
                        })?
                        && let Some(ownership) = self.paths_registry.get(&candidate)
                        && current_path_is_conda_owned(prefix, &candidate, ownership)?
                    {
                        protected_directory = true;
                        break;
                    }
                }
                if uv_will_visit_parent && (protected_directory || !cleanup_parents.is_empty()) {
                    // Removing the entry through uv would also prune parents and
                    // recursively remove __pycache__. Delete only the RECORD path.
                    protection.cleanup_sensitive.insert(record_path.to_owned());
                }
            }
        }
        Ok(protection)
    }

    /// Check if the installation of the wheels is going to clobber any installed conda package
    /// and return the names of the packages that are going to be clobbered
    /// this allow to warn the user about the overwriting of already installed packages
    /// in case of wrong mapping data
    /// or malicious packages
    pub(crate) fn clobber_on_installation(
        self,
        wheels: Vec<CachedDist>,
        venv: &PythonEnvironment,
    ) -> miette::Result<Option<ClobberReport>> {
        let Some(install_paths) = WheelInstallPaths::from_environment(venv) else {
            tracing::debug!(
                "skipping conda-clobber check: the interpreter's install scheme is not inside its sys_prefix"
            );
            return Ok(None);
        };

        let mut clobber_report = ClobberReport::default();

        for wheel in wheels {
            let pypi_package = wheel.name().to_string();
            let (records, kind) = match get_wheel_info(wheel.path()) {
                Ok(Some(whl_info)) => whl_info,
                Ok(None) => {
                    tracing::debug!(
                        "skipping conda-clobber check for '{pypi_package}': unknown wheel layout"
                    );
                    continue;
                }
                Err(err) => {
                    tracing::debug!(
                        "skipping conda-clobber check for '{pypi_package}': failed to read wheel info: {err}"
                    );
                    continue;
                }
            };

            // Important limitation:
            //
            // This check is based on files listed in the wheel RECORD before
            // installation. It therefore covers files that are physically present
            // in the wheel archive, including PEP 427 `.data/<scheme>/...` files.
            //
            // It does *not* currently cover scripts generated by the installer from
            // `<dist>.dist-info/entry_points.txt` (`console_scripts` / `gui_scripts`).
            // Those files are not present in the pre-install wheel RECORD. Covering
            // them requires parsing entry_points.txt and mirroring uv's generated
            // script/launcher filenames for the target platform.
            //
            // We decided to postpone this to a later point, as this check is going
            // to be relatively expensive. Let's revisit if we have a user hit this in the future.
            for entry in records {
                let Some(path_to_clobber) =
                    CondaPrefixPath::from_wheel_record(&install_paths, kind, entry.path)
                else {
                    continue;
                };

                if let Some(ownership) = self.paths_registry.get(&path_to_clobber) {
                    clobber_report
                        .entry((
                            pypi_package.clone(),
                            ownership.package_name.as_normalized().to_string(),
                        ))
                        .or_default()
                        .push(path_to_clobber);
                }
            }
        }
        if clobber_report.is_empty() {
            return Ok(None);
        }
        Ok(Some(clobber_report))
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use rattler_conda_types::{
        PackageName, PackageRecord, PrefixRecord, RepoDataRecord, Version,
        package::{CondaArchiveType, DistArchiveIdentifier},
        prefix_record::{PathType, PathsEntry},
    };
    use url::Url;
    use uv_install_wheel::RecordEntry;

    use super::{
        ClobberReport, CondaPrefixPath, WheelDataScheme, WheelInstallPaths, parse_wheel_data_path,
    };
    use crate::install_wheel::LibKind;

    /// All destinations are prefix-relative, mirroring what
    /// `WheelInstallPaths::from_environment` produces.
    fn install_paths() -> WheelInstallPaths {
        WheelInstallPaths {
            purelib: PathBuf::from("lib/python3.12/site-packages"),
            platlib: PathBuf::from("lib/python3.12/site-packages"),
            headers: PathBuf::from("include/python3.12"),
            scripts: PathBuf::from("bin"),
            data: PathBuf::from(""),
        }
    }

    fn prefix_record(paths: Vec<PathsEntry>) -> PrefixRecord {
        let package_record = PackageRecord::new(
            PackageName::new_unchecked("conda-pkg"),
            "1.0".parse::<Version>().unwrap(),
            "0".to_string(),
        );
        let identifier =
            DistArchiveIdentifier::new("conda-pkg-1.0-0".parse().unwrap(), CondaArchiveType::Conda);
        PrefixRecord::from_repodata_record(
            RepoDataRecord {
                package_record,
                identifier,
                url: Url::parse("https://example.invalid/conda-pkg-1.0-0.conda").unwrap(),
                channel: None,
            },
            paths,
        )
    }

    fn directory_entry(path: impl Into<PathBuf>) -> PathsEntry {
        PathsEntry {
            relative_path: path.into(),
            original_path: None,
            path_type: PathType::Directory,
            no_link: false,
            sha256: None,
            sha256_in_prefix: None,
            size_in_bytes: None,
            file_mode: None,
            prefix_placeholder: None,
        }
    }

    fn file_entry(path: impl Into<PathBuf>, contents: &[u8]) -> PathsEntry {
        PathsEntry {
            relative_path: path.into(),
            original_path: None,
            path_type: PathType::HardLink,
            no_link: false,
            sha256: Some(rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(contents)),
            sha256_in_prefix: None,
            size_in_bytes: Some(contents.len() as u64),
            file_mode: None,
            prefix_placeholder: None,
        }
    }

    fn record_entry(path: impl Into<String>) -> RecordEntry {
        RecordEntry {
            path: path.into(),
            hash: None,
            size: None,
        }
    }

    /// Regression test: regular wheel files (the common case) must come out
    /// in the prefix-relative form conda's `paths.json` uses. Before the fix
    /// these all failed an absolute `strip_prefix` and site-packages
    /// clobbering was never detected.
    #[test]
    fn regular_record_path_is_matched_prefix_relative() {
        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths(),
                LibKind::Pure,
                "boltons/__init__.py"
            ),
            Some(CondaPrefixPath(PathBuf::from(
                "lib/python3.12/site-packages/boltons/__init__.py"
            )))
        );
    }

    /// The wheel kind selects between the purelib and platlib destinations.
    #[test]
    fn platlib_wheel_uses_platlib_destination() {
        let install_paths = WheelInstallPaths {
            platlib: PathBuf::from("lib/python3.12/plat-packages"),
            ..install_paths()
        };

        assert_eq!(
            CondaPrefixPath::from_wheel_record(&install_paths, LibKind::Plat, "native.so"),
            Some(CondaPrefixPath(PathBuf::from(
                "lib/python3.12/plat-packages/native.so"
            )))
        );
    }

    /// The destinations come from the interpreter's actual layout, so a
    /// relocated site-packages (cf. `python_site_packages_dir`) flows through
    /// both for regular files and for relative escapes — an escape resolves
    /// against the *real* location, not a hardcoded one.
    #[test]
    fn relocated_site_packages_is_matched() {
        let install_paths = WheelInstallPaths {
            purelib: PathBuf::from("weird/place/site-packages"),
            platlib: PathBuf::from("weird/place/site-packages"),
            ..install_paths()
        };

        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths,
                LibKind::Pure,
                "boltons/__init__.py"
            ),
            Some(CondaPrefixPath(PathBuf::from(
                "weird/place/site-packages/boltons/__init__.py"
            )))
        );
        assert_eq!(
            CondaPrefixPath::from_wheel_record(&install_paths, LibKind::Pure, "../../bla"),
            Some(CondaPrefixPath(PathBuf::from("weird/bla")))
        );
    }

    /// A RECORD entry may escape *site-packages* and still land inside the
    /// prefix; that is a regular, comparable file (prek ships its binary
    /// like this).
    #[test]
    fn record_path_escaping_site_packages_is_matched_prefix_relative() {
        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths(),
                LibKind::Pure,
                "../../../bin/prek"
            ),
            Some(CondaPrefixPath(PathBuf::from("bin/prek")))
        );
    }

    #[test]
    fn installed_record_paths_are_matched_prefix_relative() {
        let prefix = PathBuf::from("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");

        assert_eq!(
            CondaPrefixPath::from_installed_wheel_record(
                &prefix,
                &site_packages,
                "boltons/__init__.py"
            ),
            Some(CondaPrefixPath(PathBuf::from(
                "lib/python3.12/site-packages/boltons/__init__.py"
            )))
        );
        assert_eq!(
            CondaPrefixPath::from_installed_wheel_record(
                &prefix,
                &site_packages,
                "../../../bin/boltons"
            ),
            Some(CondaPrefixPath(PathBuf::from("bin/boltons")))
        );
    }

    #[test]
    fn installed_record_paths_outside_prefix_are_ignored() {
        let prefix = PathBuf::from("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");

        assert_eq!(
            CondaPrefixPath::from_installed_wheel_record(
                &prefix,
                &site_packages,
                "../../../../../outside"
            ),
            None
        );
    }

    #[test]
    fn site_packages_outside_prefix_is_rejected() {
        let prefix = PathBuf::from("prefix");
        let site_packages = PathBuf::from("other/lib/python3.12/site-packages");
        let records = [record_entry("pkg/module.py")];

        let error = super::PypiCondaClobberRegistry::default()
            .conda_owned_record_paths(&prefix, &site_packages, &records)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn filesystem_aliases_are_used_for_prefix_relationships() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let real_prefix = temp_dir.path().join("real-prefix");
        let prefix_alias = temp_dir.path().join("prefix-alias");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/pkg/module.py");
        let site_packages = real_prefix.join("lib/python3.12/site-packages");
        let conda_contents = b"conda module";
        fs_err::create_dir_all(real_prefix.join(conda_path.parent().unwrap())).unwrap();
        fs_err::write(real_prefix.join(&conda_path), conda_contents).unwrap();
        symlink(&real_prefix, &prefix_alias).unwrap();
        assert_eq!(
            super::filesystem_relative_path(&prefix_alias, &site_packages).unwrap(),
            Some(PathBuf::from("lib/python3.12/site-packages"))
        );
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                directory_entry("lib/python3.12/site-packages/pkg"),
                file_entry(&conda_path, conda_contents),
            ])]);
        let records = [record_entry("pkg/module.py")];

        let protection = registry
            .conda_owned_record_paths(&prefix_alias, &site_packages, &records)
            .unwrap();

        assert!(protection.owned.contains("pkg/module.py"));
        assert!(
            registry
                .current_directory_is_conda_owned(&prefix_alias, &site_packages.join("pkg"))
                .unwrap()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unicode_normalized_prefix_alias_is_used_for_site_packages() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("pr\u{e9}fix");
        let site_packages = temp_dir
            .path()
            .join("pre\u{301}fix/lib/python3.12/site-packages");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/pkg/module.py");
        let conda_contents = b"conda module";
        fs_err::create_dir_all(prefix.join(conda_path.parent().unwrap())).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        if fs_err::symlink_metadata(&site_packages).is_err() {
            eprintln!(
                "skipping Unicode normalization test on a normalization-sensitive filesystem"
            );
            return;
        }
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, conda_contents),
            ])]);
        let records = [record_entry("pkg/module.py")];

        let protection = registry
            .conda_owned_record_paths(&prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.owned.contains("pkg/module.py"));
    }

    #[cfg(windows)]
    #[test]
    fn installed_absolute_record_path_inside_prefix_is_matched() {
        let prefix = PathBuf::from(r"C:\prefix");
        let site_packages = prefix.join(r"Lib\site-packages");

        assert_eq!(
            CondaPrefixPath::from_installed_wheel_record(
                &prefix,
                &site_packages,
                r"C:\prefix\Lib\site-packages\pkg\__init__.py"
            ),
            Some(CondaPrefixPath(PathBuf::from(
                r"Lib\site-packages\pkg\__init__.py"
            )))
        );
    }

    #[test]
    fn record_cleanup_does_not_visit_conda_owned_pycache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let pycache_parent = PathBuf::from("lib/python3.12/site-packages/pkg");
        let pyc_path = pycache_parent.join("__pycache__/module.cpython-312.pyc");
        let source_path = pycache_parent.join("__init__.py");
        let conda_source = b"conda source";
        let conda_pyc = b"conda bytecode";
        fs_err::create_dir_all(prefix.join(pyc_path.parent().unwrap())).unwrap();
        fs_err::write(prefix.join(&source_path), conda_source).unwrap();
        fs_err::write(prefix.join(&pyc_path), conda_pyc).unwrap();
        fs_err::write(site_packages.join("pkg/other.py"), b"wheel module").unwrap();
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&source_path, conda_source),
                file_entry(&pyc_path, conda_pyc),
            ])]);
        let records = [
            record_entry("pkg/__init__.py"),
            record_entry("pkg/other.py"),
            record_entry("unrelated.py"),
        ];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.owned.contains("pkg/__init__.py"));
        assert!(protection.cleanup_sensitive.contains("pkg/other.py"));
        assert!(!protection.cleanup_sensitive.contains("unrelated.py"));
        assert!(
            protection
                .protected_pycache_paths
                .get(Path::new("lib/python3.12/site-packages/pkg"))
                .is_some_and(|paths| paths.contains(&prefix.join(&pyc_path)))
        );
    }

    #[test]
    fn record_cleanup_ignores_modified_conda_pycache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let pycache_parent = PathBuf::from("lib/python3.12/site-packages/pkg");
        let pyc_path = pycache_parent.join("__pycache__/module.cpython-312.pyc");
        let conda_pyc = b"conda bytecode";
        fs_err::create_dir_all(prefix.join(pyc_path.parent().unwrap())).unwrap();
        fs_err::write(prefix.join(&pyc_path), conda_pyc).unwrap();
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&pyc_path, conda_pyc),
            ])]);
        fs_err::write(prefix.join(&pyc_path), b"modified bytecode").unwrap();
        fs_err::write(site_packages.join("pkg/other.py"), b"wheel module").unwrap();
        let records = [record_entry("pkg/other.py")];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(!protection.cleanup_sensitive.contains("pkg/other.py"));
        assert!(
            protection
                .protected_pycache_paths
                .get(pycache_parent.as_path())
                .is_some_and(|paths| paths.is_empty())
        );
        let packages = registry
            .packages_requiring_reinstall_for_records(prefix, &site_packages, records.iter())
            .unwrap();
        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));
    }

    #[test]
    fn missing_record_uses_existing_parent_to_find_missing_conda_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/pkg/conda.py");
        fs_err::create_dir_all(prefix.join(conda_path.parent().unwrap())).unwrap();
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, b"conda module"),
            ])]);
        let records = [record_entry("pkg/missing.py")];

        let packages = registry
            .packages_requiring_reinstall_for_records(prefix, &site_packages, records.iter())
            .unwrap();

        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));

        fs_err::write(prefix.join(&conda_path), b"conda module").unwrap();
        let packages = registry
            .packages_requiring_reinstall_for_records(prefix, &site_packages, records.iter())
            .unwrap();
        assert!(packages.is_empty());
    }

    #[test]
    fn record_cleanup_does_not_prune_conda_owned_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        fs_err::create_dir_all(site_packages.join("pkg")).unwrap();
        fs_err::write(site_packages.join("pkg/file.py"), b"wheel module").unwrap();
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                directory_entry("lib/python3.12/site-packages/pkg"),
            ])]);
        let records = [record_entry("pkg/file.py"), record_entry("unrelated.py")];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.cleanup_sensitive.contains("pkg/file.py"));
        assert!(!protection.cleanup_sensitive.contains("unrelated.py"));
    }

    #[test]
    fn directory_record_does_not_remove_conda_owned_descendant() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/pkg/module.py");
        let conda_contents = b"conda module";
        fs_err::create_dir_all(prefix.join(conda_path.parent().unwrap())).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, conda_contents),
            ])]);
        let records = [record_entry("pkg/")];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.unsafe_to_remove.contains("pkg/"));

        fs_err::write(prefix.join(&conda_path), b"modified by pypi").unwrap();
        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();
        assert!(!protection.unsafe_to_remove.contains("pkg/"));
        let packages = registry
            .packages_requiring_reinstall_for_records(prefix, &site_packages, records.iter())
            .unwrap();
        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));
    }

    #[test]
    fn directory_record_escaping_prefix_is_unsafe() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let conda_path = PathBuf::from("outside/module.py");
        let conda_contents = b"conda module";
        fs_err::create_dir_all(&site_packages).unwrap();
        fs_err::create_dir_all(prefix.join(conda_path.parent().unwrap())).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        fs_err::create_dir_all(temp_dir.path().join("outside")).unwrap();
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, conda_contents),
            ])]);
        let records = [record_entry("../../../../outside/")];

        let protection = registry
            .conda_owned_record_paths(&prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.owned.is_empty());
        assert!(protection.unsafe_to_remove.contains("../../../../outside/"));
        assert!(protection.cleanup_sensitive.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn case_folded_parent_collision_does_not_protect_distinct_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let conda_root = PathBuf::from("lib/python3.12/site-packages/MixedCase");
        let wheel_root = PathBuf::from("lib/python3.12/site-packages/mixedcase");
        let conda_path = conda_root.join("module.py");
        let pyc_path = conda_root.join("__pycache__/conda.pyc");
        let conda_contents = b"conda module";
        let conda_pyc = b"conda bytecode";
        fs_err::create_dir_all(prefix.join(pyc_path.parent().unwrap())).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        fs_err::write(prefix.join(&pyc_path), conda_pyc).unwrap();
        if fs_err::symlink_metadata(prefix.join(&wheel_root)).is_ok() {
            eprintln!("skipping distinct-case test on a case-insensitive filesystem");
            return;
        }
        fs_err::create_dir_all(prefix.join(&wheel_root)).unwrap();
        fs_err::write(prefix.join(&wheel_root).join("other.py"), b"wheel").unwrap();
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, conda_contents),
                file_entry(&pyc_path, conda_pyc),
            ])]);
        let records = [
            record_entry("mixedcase/other.py"),
            record_entry("mixedcase/"),
        ];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(!protection.cleanup_sensitive.contains("mixedcase/other.py"));
        assert!(!protection.unsafe_to_remove.contains("mixedcase/"));
        assert!(protection.protected_pycache_paths.is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unicode_normalized_directory_alias_preserves_conda_descendant() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let composed_root = PathBuf::from("lib/python3.12/site-packages/caf\u{e9}");
        let decomposed_record = "cafe\u{301}/";
        let conda_path = composed_root.join("module.py");
        let conda_contents = b"conda module";
        fs_err::create_dir_all(prefix.join(&composed_root)).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();

        let decomposed_root = site_packages.join("cafe\u{301}");
        if fs_err::symlink_metadata(&decomposed_root).is_err() {
            eprintln!(
                "skipping Unicode normalization test on a normalization-sensitive filesystem"
            );
            return;
        }

        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, conda_contents),
            ])]);
        let records = [record_entry(decomposed_record)];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.unsafe_to_remove.contains(decomposed_record));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_unicode_normalized_record_path_requires_conda_relink() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let composed_root = PathBuf::from("lib/python3.12/site-packages/caf\u{e9}");
        let conda_path = composed_root.join("modul\u{e9}.py");
        fs_err::create_dir_all(prefix.join(&composed_root)).unwrap();

        let decomposed_root = site_packages.join("cafe\u{301}");
        if fs_err::symlink_metadata(&decomposed_root).is_err() {
            eprintln!(
                "skipping Unicode normalization test on a normalization-sensitive filesystem"
            );
            return;
        }

        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, b"conda module"),
            ])]);
        let records = [record_entry("cafe\u{301}/module\u{301}.py")];

        let packages = registry
            .packages_requiring_reinstall_for_records(prefix, &site_packages, records.iter())
            .unwrap();

        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));
    }

    #[cfg(windows)]
    #[test]
    fn case_insensitive_parent_aliases_preserve_conda_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("Lib/site-packages");
        let package_root = PathBuf::from("Lib/site-packages/MixedCase");
        let pyc_path = package_root.join("__pycache__/conda.pyc");
        let conda_directory = package_root.join("CondaDir");
        let conda_pyc = b"conda bytecode";
        fs_err::create_dir_all(prefix.join(pyc_path.parent().unwrap())).unwrap();
        fs_err::create_dir_all(prefix.join(&conda_directory)).unwrap();
        fs_err::write(prefix.join(&pyc_path), conda_pyc).unwrap();
        if fs_err::symlink_metadata(prefix.join("lib/site-packages/mixedcase")).is_err() {
            eprintln!("skipping case-insensitive test on a case-sensitive filesystem");
            return;
        }
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&pyc_path, conda_pyc),
                directory_entry(&conda_directory),
            ])]);
        let records = [
            record_entry("mixedcase/other.py"),
            record_entry("mixedcase/condadir/other.py"),
        ];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.cleanup_sensitive.contains("mixedcase/other.py"));
        assert!(
            protection
                .cleanup_sensitive
                .contains("mixedcase/condadir/other.py")
        );
        assert!(
            protection
                .protected_pycache_paths
                .get(package_root.as_path())
                .is_some_and(|paths| paths.contains(&prefix.join(&pyc_path)))
        );
    }

    #[cfg(windows)]
    #[test]
    fn record_paths_use_canonical_case_for_ownership_lookup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("Lib/site-packages");
        let conda_path = PathBuf::from("Lib/site-packages/MixedCase/module.py");
        let conda_contents = b"conda file";
        fs_err::create_dir_all(prefix.join(conda_path.parent().unwrap())).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        if fs_err::symlink_metadata(prefix.join("lib/site-packages/mixedcase/MODULE.py")).is_err() {
            eprintln!("skipping case-insensitive test on a case-sensitive filesystem");
            return;
        }
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, conda_contents),
            ])]);
        let records = [record_entry("mixedcase/MODULE.py")];

        let protection = registry
            .conda_owned_record_paths(prefix, &site_packages, &records)
            .unwrap();

        assert!(protection.owned.contains("mixedcase/MODULE.py"));
    }

    #[cfg(windows)]
    #[test]
    fn missing_case_aliased_record_path_requires_conda_relink() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("Lib/site-packages");
        let conda_path = PathBuf::from("Lib/site-packages/MixedCase/Module.py");
        fs_err::create_dir_all(prefix.join(conda_path.parent().unwrap())).unwrap();
        if fs_err::symlink_metadata(prefix.join("lib/site-packages/mixedcase")).is_err() {
            eprintln!("skipping case-insensitive test on a case-sensitive filesystem");
            return;
        }
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, b"conda module"),
            ])]);
        let records = [record_entry("mixedcase/module.py")];

        let packages = registry
            .packages_requiring_reinstall_for_records(prefix, &site_packages, records.iter())
            .unwrap();

        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));
    }

    #[cfg(windows)]
    #[test]
    fn short_name_record_path_requires_conda_relink() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("Lib/site-packages");
        let long_directory = site_packages.join("LongPackageDirectory");
        fs_err::create_dir_all(&long_directory).unwrap();
        let command = format!(
            r#"for %I in ("{}") do @echo %~sI"#,
            long_directory.display()
        );
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", &command])
            .output()
            .unwrap();
        let short_path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        let Some(short_name) = short_path.file_name().and_then(|name| name.to_str()) else {
            eprintln!("skipping 8.3 alias test because no short name was returned");
            return;
        };
        if !short_name.contains('~') {
            eprintln!("skipping 8.3 alias test because short names are disabled");
            return;
        }

        let conda_path = PathBuf::from("Lib/site-packages/LongPackageDirectory/Module.py");
        let registry =
            super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record(vec![
                file_entry(&conda_path, b"conda module"),
            ])]);
        let records = [record_entry(format!("{short_name}/Module.py"))];

        let packages = registry
            .packages_requiring_reinstall_for_records(prefix, &site_packages, records.iter())
            .unwrap();

        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));
    }

    #[test]
    fn case_folded_lookup_normalizes_directory_separators() {
        let forward_slashes =
            CondaPrefixPath(PathBuf::from("Lib/site-packages/MixedCase/module.py"));
        let backward_slashes =
            CondaPrefixPath(PathBuf::from(r"Lib\site-packages\mixedcase\MODULE.py"));

        assert_eq!(
            super::case_folded_path_hash(&forward_slashes),
            super::case_folded_path_hash(&backward_slashes)
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonical_lookup_compares_symlink_directory_entries_without_following_them() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        fs_err::write(prefix.join("target"), b"target").unwrap();
        symlink("target", prefix.join("Module.py")).unwrap();

        let path = CondaPrefixPath(PathBuf::from("Module.py"));
        assert!(super::canonical_paths_match(prefix, &path, &path).unwrap());

        if fs_err::symlink_metadata(prefix.join("module.py")).is_ok() {
            eprintln!("skipping case-sensitive symlink test on a case-insensitive filesystem");
            return;
        }
        symlink("target", prefix.join("module.py")).unwrap();
        let case_variant = CondaPrefixPath(PathBuf::from("module.py"));
        assert!(
            !super::canonical_paths_match(prefix, &path, &case_variant).unwrap(),
            "distinct case-sensitive symlinks must not be treated as one entry"
        );

        fs_err::write(prefix.join("hardlink-a"), b"hardlink").unwrap();
        fs_err::hard_link(prefix.join("hardlink-a"), prefix.join("hardlink-b")).unwrap();
        let hardlink_a = CondaPrefixPath(PathBuf::from("hardlink-a"));
        let hardlink_b = CondaPrefixPath(PathBuf::from("hardlink-b"));
        assert!(
            !super::canonical_paths_match(prefix, &hardlink_a, &hardlink_b).unwrap(),
            "distinct hardlinks must not be treated as one directory entry"
        );
    }

    #[cfg(unix)]
    #[test]
    fn current_conda_ownership_checks_path_type_and_hash() {
        use std::os::unix::fs::symlink;

        use super::CondaPathOwnership;

        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let path = CondaPrefixPath(PathBuf::from("claimed"));
        let package_name = PackageName::new_unchecked("conda-pkg");
        let file_contents = b"conda file";
        let file_hash =
            rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(file_contents);
        fs_err::write(prefix.join(path.as_path()), file_contents).unwrap();

        let file_ownership = CondaPathOwnership {
            package_name: package_name.clone(),
            path_type: PathType::HardLink,
            expected_sha256: Some(file_hash),
        };
        assert!(super::current_path_is_conda_owned(prefix, &path, &file_ownership).unwrap());
        fs_err::write(prefix.join(path.as_path()), b"wheel file").unwrap();
        assert!(!super::current_path_is_conda_owned(prefix, &path, &file_ownership).unwrap());

        let target = prefix.join("target");
        fs_err::write(&target, file_contents).unwrap();
        fs_err::remove_file(prefix.join(path.as_path())).unwrap();
        symlink("target", prefix.join(path.as_path())).unwrap();
        assert!(
            !super::current_path_is_conda_owned(prefix, &path, &file_ownership).unwrap(),
            "a symlink must not satisfy a regular-file PrefixRecord entry"
        );

        let symlink_ownership = CondaPathOwnership {
            package_name: package_name.clone(),
            path_type: PathType::SoftLink,
            expected_sha256: Some(
                rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(b"target"),
            ),
        };
        assert!(super::current_path_is_conda_owned(prefix, &path, &symlink_ownership).unwrap());
        let wrong_symlink_ownership = CondaPathOwnership {
            expected_sha256: Some(
                rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(b"other-target"),
            ),
            ..symlink_ownership
        };
        assert!(
            !super::current_path_is_conda_owned(prefix, &path, &wrong_symlink_ownership).unwrap()
        );

        fs_err::remove_file(prefix.join(path.as_path())).unwrap();
        fs_err::copy(&target, prefix.join(path.as_path())).unwrap();
        let copied_symlink_ownership = CondaPathOwnership {
            package_name: package_name.clone(),
            path_type: PathType::SoftLink,
            expected_sha256: Some(file_hash),
        };
        assert!(
            super::current_path_is_conda_owned(prefix, &path, &copied_symlink_ownership).unwrap(),
            "a copied symlink target must satisfy its in-prefix hash"
        );

        fs_err::remove_file(prefix.join(path.as_path())).unwrap();
        symlink("target", prefix.join(path.as_path())).unwrap();

        let directory_ownership = CondaPathOwnership {
            package_name,
            path_type: PathType::Directory,
            expected_sha256: None,
        };
        assert!(
            !super::current_path_is_conda_owned(prefix, &path, &directory_ownership).unwrap(),
            "a directory symlink must not satisfy a Directory PrefixRecord entry"
        );

        let real_directory = prefix.join("real-directory");
        fs_err::create_dir(&real_directory).unwrap();
        fs_err::write(real_directory.join("nested"), file_contents).unwrap();
        symlink("real-directory", prefix.join("directory-link")).unwrap();
        let nested_path = CondaPrefixPath(PathBuf::from("directory-link/nested"));
        assert!(
            !super::current_path_is_conda_owned(prefix, &nested_path, &file_ownership).unwrap(),
            "ownership checks must not follow a symlinked ancestor directory"
        );
    }

    /// Entries that escape the *prefix* (or are absolute) cannot be expressed
    /// in conda's prefix-relative form and are skipped.
    #[test]
    fn record_path_outside_prefix_is_ignored() {
        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths(),
                LibKind::Pure,
                "../../../../../bin/prek"
            ),
            None
        );
        assert_eq!(
            CondaPrefixPath::from_wheel_record(&install_paths(), LibKind::Pure, "/abs/evil"),
            None
        );
        // On Windows a path can also be root-relative (`\abs\evil`, no drive
        // prefix, not `is_absolute()`) or carry a drive prefix; both must be
        // rejected too.
        #[cfg(windows)]
        {
            assert_eq!(
                CondaPrefixPath::from_wheel_record(&install_paths(), LibKind::Pure, "\\abs\\evil"),
                None
            );
            assert_eq!(
                CondaPrefixPath::from_wheel_record(
                    &install_paths(),
                    LibKind::Pure,
                    "C:\\abs\\evil"
                ),
                None
            );
        }
    }

    #[test]
    fn parses_pep427_data_scheme_paths() {
        assert_eq!(
            parse_wheel_data_path(std::path::Path::new("prek-0.4.4.data/scripts/prek")),
            Some((WheelDataScheme::Scripts, std::path::Path::new("prek")))
        );
        assert_eq!(
            parse_wheel_data_path(std::path::Path::new("pkg-1.0.data/purelib/module.py")),
            Some((WheelDataScheme::Purelib, std::path::Path::new("module.py")))
        );
        assert_eq!(
            parse_wheel_data_path(std::path::Path::new("prek/__init__.py")),
            None
        );
    }

    #[test]
    fn wheel_data_scheme_paths_are_matched_prefix_relative() {
        let install_paths = install_paths();

        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths,
                LibKind::Pure,
                "prek-0.4.4.data/scripts/prek"
            ),
            Some(CondaPrefixPath(PathBuf::from("bin/prek")))
        );
        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths,
                LibKind::Pure,
                "pkg-1.0.data/purelib/module.py"
            ),
            Some(CondaPrefixPath(PathBuf::from(
                "lib/python3.12/site-packages/module.py"
            )))
        );
        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths,
                LibKind::Pure,
                "pkg-1.0.data/headers/pkg.h"
            ),
            Some(CondaPrefixPath(PathBuf::from("include/python3.12/pkg.h")))
        );
        assert_eq!(
            CondaPrefixPath::from_wheel_record(
                &install_paths,
                LibKind::Pure,
                "pkg-1.0.data/data/share/pkg/data.txt"
            ),
            Some(CondaPrefixPath(PathBuf::from("share/pkg/data.txt")))
        );
    }

    #[test]
    fn clobber_warning_groups_by_package_and_limits_files() {
        let mut report = ClobberReport::default();
        report
            .entry(("prek".to_string(), "prek".to_string()))
            .or_default()
            .extend((1..=7).map(|idx| CondaPrefixPath(PathBuf::from(format!("bin/prek-{idx}")))));

        assert_eq!(
            report.to_string(),
            "PyPI package files will overwrite files installed by conda packages:\n  - PyPI package 'prek' overwrites conda package 'prek':\n    - bin/prek-1\n    - bin/prek-2\n    - bin/prek-3\n    - bin/prek-4\n    - bin/prek-5\n    - ... 2 other files\n"
        );
    }
}
