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
    /// Pycache paths grouped by the parent directory whose cleanup uv would visit.
    protected_pycache_paths: AHashMap<PathBuf, AHashSet<CondaPrefixPath>>,
    /// Candidate paths indexed by a case-folded hash. Canonical identity is
    /// still checked before a candidate is used.
    case_folded_paths: AHashMap<u64, Vec<CondaPrefixPath>>,
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
    pub(crate) cleanup_sensitive: AHashSet<String>,
    pub(crate) protected_pycache_paths: AHashMap<PathBuf, AHashSet<PathBuf>>,
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
                Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(err) => return Err(err),
            }
        }
    }
    Ok(false)
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
        .to_lowercase()
        .hash(&mut hasher);
    hasher.finish()
}

fn canonical_paths_match(
    prefix: &Path,
    left: &CondaPrefixPath,
    right: &CondaPrefixPath,
) -> io::Result<bool> {
    if has_symlink_ancestor(prefix, left)? || has_symlink_ancestor(prefix, right)? {
        return Ok(false);
    }

    for path in [left, right] {
        match fs_err::symlink_metadata(prefix.join(path.as_path())) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(false),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err),
        }
    }

    Ok(fs_err::canonicalize(prefix.join(left.as_path()))?
        == fs_err::canonicalize(prefix.join(right.as_path()))?)
}

impl PypiCondaClobberRegistry {
    /// Register the paths of the installed conda packages
    /// to later check if they are going to be clobbered by the installation of the wheels
    pub(crate) fn with_conda_packages(conda_packages: &[PrefixRecord]) -> Self {
        let mut registry = AHashMap::with_capacity(conda_packages.len() * 50);
        let mut protected_directories = AHashSet::new();
        let mut protected_pycache_paths = AHashMap::<PathBuf, AHashSet<CondaPrefixPath>>::new();
        let mut case_folded_paths = AHashMap::<u64, Vec<CondaPrefixPath>>::new();
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
                case_folded_paths
                    .entry(case_folded_path_hash(&path))
                    .or_default()
                    .push(path.clone());
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
            protected_pycache_paths,
            case_folded_paths,
        }
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

        let pycache_root = parent.join("__pycache__");
        let mut protected_paths = AHashSet::new();
        if let Some(conda_paths) = self.protected_pycache_paths.get(parent) {
            for conda_path in conda_paths {
                let Some(ownership) = self.paths_registry.get(conda_path) else {
                    continue;
                };
                if current_path_is_conda_owned(prefix, conda_path, ownership)?
                    && let Ok(relative_path) = conda_path.as_path().strip_prefix(&pycache_root)
                {
                    protected_paths.insert(relative_path.to_path_buf());
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
        if !site_packages.starts_with(prefix) {
            tracing::debug!(
                "skipping conda-owned RECORD path lookup: site-packages {} is not inside prefix {}",
                site_packages.display(),
                prefix.display()
            );
            return Ok(CondaRecordPathProtection::default());
        }

        let mut protection = CondaRecordPathProtection::default();
        for record in records {
            let record_path = record.path.as_str();
            let Some(mut path) =
                CondaPrefixPath::from_installed_wheel_record(prefix, site_packages, record_path)
            else {
                continue;
            };

            if !self.paths_registry.contains_key(&path)
                && let Some(candidates) = self.case_folded_paths.get(&case_folded_path_hash(&path))
            {
                for candidate in candidates {
                    if canonical_paths_match(prefix, &path, candidate)? {
                        path = candidate.clone();
                        break;
                    }
                }
            }

            let cleanup_parents = path
                .as_path()
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .filter(|ancestor| self.protected_pycache_paths.contains_key(*ancestor))
                .map(Path::to_path_buf)
                .collect::<Vec<_>>();

            let owned = if let Some(ownership) = self.paths_registry.get(&path) {
                current_path_is_conda_owned(prefix, &path, ownership)?
            } else {
                false
            };
            if owned {
                protection.owned.insert(record_path.to_owned());
            } else {
                let mut protected_directory = false;
                for ancestor in path.as_path().ancestors() {
                    let candidate = CondaPrefixPath((*ancestor).to_path_buf());
                    if self.protected_directories.contains(&candidate)
                        && let Some(ownership) = self.paths_registry.get(&candidate)
                        && current_path_is_conda_owned(prefix, &candidate, ownership)?
                    {
                        protected_directory = true;
                        break;
                    }
                }
                if protected_directory || !cleanup_parents.is_empty() {
                    // Removing the entry through uv would also prune parents and
                    // recursively remove __pycache__. Delete only the RECORD path.
                    protection.cleanup_sensitive.insert(record_path.to_owned());
                }
            }

            if protection.owned.contains(record_path)
                || protection.cleanup_sensitive.contains(record_path)
            {
                for parent in cleanup_parents {
                    self.protect_pycache_paths(prefix, &parent, &mut protection)?;
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
                .is_some_and(|paths| paths.contains(Path::new("module.cpython-312.pyc")))
        );
    }

    #[test]
    fn record_cleanup_does_not_prune_conda_owned_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path();
        let site_packages = prefix.join("lib/python3.12/site-packages");
        fs_err::create_dir_all(site_packages.join("pkg")).unwrap();
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

        symlink("target", prefix.join("module.py")).unwrap();
        let case_variant = CondaPrefixPath(PathBuf::from("module.py"));
        assert!(
            !super::canonical_paths_match(prefix, &path, &case_variant).unwrap(),
            "distinct case-sensitive symlinks must not be treated as one entry"
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
