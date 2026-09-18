use std::{
    collections::{BTreeMap, btree_map},
    fmt,
    path::{Path, PathBuf},
};

use pixi_path::normalize_std;
use rattler_conda_types::PrefixRecord;
use uv_distribution_types::{CachedDist, Name};
use uv_python::PythonEnvironment;

use ahash::AHashMap;
use miette::IntoDiagnostic;

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
    paths_registry: AHashMap<CondaPrefixPath, rattler_conda_types::PackageName>,
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
    /// From a conda `PrefixRecord` path, which should be prefix-relative by
    /// definition. Returns `None` for a malformed (non-relative) entry: such
    /// a key could never match a wheel-side path anyway, and the clobber
    /// check is best-effort.
    fn from_conda_record(path: PathBuf) -> Option<Self> {
        if path.is_relative() {
            Some(Self(normalize_std(&path)))
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
        match path.components().next() {
            Some(std::path::Component::Normal(_)) => Some(Self(path)),
            _ => None,
        }
    }

    fn as_path(&self) -> &Path {
        &self.0
    }
}

impl PypiCondaClobberRegistry {
    /// Register the paths of the installed conda packages
    /// to later check if they are going to be clobbered by the installation of the wheels
    pub(crate) fn with_conda_packages(conda_packages: &[PrefixRecord]) -> Self {
        let mut registry = AHashMap::with_capacity(conda_packages.len() * 50);
        for record in conda_packages {
            for path in &record.paths_data.paths {
                let Some(path) = CondaPrefixPath::from_conda_record(path.relative_path.clone())
                else {
                    continue;
                };
                registry.insert(path, record.repodata_record.package_record.name.clone());
            }
        }
        Self {
            paths_registry: registry,
        }
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

                if let Some(name) = self.paths_registry.get(&path_to_clobber) {
                    clobber_report
                        .entry((pypi_package.clone(), name.as_normalized().to_string()))
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

    /// Return the name of the conda package claiming this path, if any.
    pub(crate) fn claimed_by_conda_package(
        &self,
        prefix_root: &Path,
        site_packages: &Path,
        record_entry_path: &str,
    ) -> Option<&rattler_conda_types::PackageName> {
        if self.paths_registry.is_empty() {
            return None;
        }

        let absolute_target = site_packages.join(record_entry_path);
        let normalized = normalize_std(&absolute_target);
        let rel = match normalized.strip_prefix(prefix_root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => {
                if let (Ok(canon_root), Ok(canon_site_packages)) = (
                    dunce::canonicalize(prefix_root),
                    dunce::canonicalize(site_packages),
                ) {
                    let canon_target = normalize_std(&canon_site_packages.join(record_entry_path));
                    if let Ok(rel) = canon_target.strip_prefix(&canon_root) {
                        rel.to_path_buf()
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
        };

        let conda_path = CondaPrefixPath::from_conda_record(rel)?;
        self.paths_registry.get(&conda_path)
    }

    /// Check if a RECORD entry path is claimed by any installed conda package.
    #[allow(dead_code)]
    pub(crate) fn is_claimed_by_conda(
        &self,
        prefix_root: &Path,
        site_packages: &Path,
        record_entry_path: &str,
    ) -> bool {
        self.claimed_by_conda_package(prefix_root, site_packages, record_entry_path)
            .is_some()
    }
}

/// Filters a PyPI package's `RECORD` file in place, removing any entries that are
/// claimed by installed conda packages in the environment.
///
/// When moving a package from PyPI to conda or unlinking an extraneous PyPI package
/// whose files overlap with an installed conda package, `uv_installer::uninstall`
/// would otherwise walk the old RECORD and delete the conda package's files from disk.
/// By removing those entries from RECORD beforehand, `uv_installer::uninstall`
/// preserves all conda-managed files while removing the PyPI `.dist-info` and any
/// unshared files.
pub(crate) fn filter_record_conda_paths(
    dist_info_path: &Path,
    prefix_root: &Path,
    clobber_registry: &PypiCondaClobberRegistry,
) -> miette::Result<()> {
    if clobber_registry.paths_registry.is_empty() {
        return Ok(());
    }

    let Some(site_packages) = dist_info_path.parent() else {
        return Ok(());
    };

    let record_path = dist_info_path.join("RECORD");
    let Ok(mut record_file) = fs_err::File::open(&record_path) else {
        return Ok(());
    };

    let records = match super::install_wheel::read_record_file(&mut record_file) {
        Ok(records) => records,
        Err(err) => {
            tracing::debug!(
                "Could not parse RECORD file `{}` to check for conda clobbering: {err}",
                record_path.display()
            );
            return Ok(());
        }
    };

    let mut any_claimed = false;
    let mut filtered_records = Vec::with_capacity(records.len());
    for entry in records {
        if let Some(conda_pkg) =
            clobber_registry.claimed_by_conda_package(prefix_root, site_packages, &entry.path)
        {
            tracing::debug!(
                "Preserving file '{}' claimed by installed conda package '{}' during PyPI uninstall",
                entry.path,
                conda_pkg.as_source()
            );
            any_claimed = true;
        } else {
            filtered_records.push(entry);
        }
    }

    if !any_claimed {
        return Ok(());
    }

    let file = fs_err::File::create(&record_path).into_diagnostic()?;
    let mut record_writer = csv::WriterBuilder::new()
        .has_headers(false)
        .escape(b'"')
        .from_writer(file);

    for entry in filtered_records {
        record_writer.serialize(entry).into_diagnostic()?;
    }
    record_writer.flush().into_diagnostic()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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

    fn mock_clobber_registry(
        paths: impl IntoIterator<Item = (&'static str, &'static str)>,
    ) -> super::PypiCondaClobberRegistry {
        use std::str::FromStr;
        let mut registry = ahash::AHashMap::default();
        for (path, pkg) in paths {
            registry.insert(
                CondaPrefixPath(PathBuf::from(path)),
                rattler_conda_types::PackageName::from_str(pkg).unwrap(),
            );
        }
        super::PypiCondaClobberRegistry {
            paths_registry: registry,
        }
    }

    #[test]
    fn test_is_claimed_by_conda() {
        let prefix = PathBuf::from("/my/env");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let registry = mock_clobber_registry([
            ("lib/python3.12/site-packages/pystac/__init__.py", "pystac"),
            ("lib/python3.12/site-packages/pystac/item.py", "pystac"),
            ("bin/pystac-cli", "pystac"),
        ]);

        assert!(registry.is_claimed_by_conda(&prefix, &site_packages, "pystac/__init__.py"));
        assert!(registry.is_claimed_by_conda(&prefix, &site_packages, "pystac/item.py"));
        assert!(registry.is_claimed_by_conda(&prefix, &site_packages, "../../../bin/pystac-cli"));

        assert!(!registry.is_claimed_by_conda(&prefix, &site_packages, "pystac/unclaimed.py"));
        assert!(!registry.is_claimed_by_conda(&prefix, &site_packages, "other/__init__.py"));
        assert!(!registry.is_claimed_by_conda(
            &prefix,
            &site_packages,
            "pystac-1.15.2.dist-info/RECORD"
        ));
        assert!(!registry.is_claimed_by_conda(
            &prefix,
            &site_packages,
            "../../../../../outside/file"
        ));
    }

    #[test]
    fn test_filter_record_conda_paths_removes_claimed_entries() {
        use crate::install_wheel::read_record_file;

        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("env");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let dist_info = site_packages.join("pystac-1.15.2.dist-info");
        fs_err::create_dir_all(&dist_info).unwrap();

        let record_path = dist_info.join("RECORD");
        let initial_record_content = "\
pystac/__init__.py,sha256=aaa,100
pystac/item.py,sha256=bbb,200
pystac_extra.py,sha256=ccc,300
pystac-1.15.2.dist-info/RECORD,,
";
        fs_err::write(&record_path, initial_record_content).unwrap();

        let registry = mock_clobber_registry([
            ("lib/python3.12/site-packages/pystac/__init__.py", "pystac"),
            ("lib/python3.12/site-packages/pystac/item.py", "pystac"),
        ]);

        super::filter_record_conda_paths(&dist_info, &prefix, &registry).unwrap();

        let mut record_file = fs_err::File::open(&record_path).unwrap();
        let records = read_record_file(&mut record_file).unwrap();

        let paths: Vec<String> = records.into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            vec![
                "pystac_extra.py".to_string(),
                "pystac-1.15.2.dist-info/RECORD".to_string()
            ]
        );
    }

    #[test]
    fn test_filter_record_conda_paths_no_claimed_leaves_file_intact() {
        use crate::install_wheel::read_record_file;

        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("env");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let dist_info = site_packages.join("foo-1.0.0.dist-info");
        fs_err::create_dir_all(&dist_info).unwrap();

        let record_path = dist_info.join("RECORD");
        let initial_record_content = "\
foo/__init__.py,sha256=aaa,100
foo-1.0.0.dist-info/RECORD,,
";
        fs_err::write(&record_path, initial_record_content).unwrap();

        let registry =
            mock_clobber_registry([("lib/python3.12/site-packages/bar/__init__.py", "bar")]);

        super::filter_record_conda_paths(&dist_info, &prefix, &registry).unwrap();

        let mut record_file = fs_err::File::open(&record_path).unwrap();
        let records = read_record_file(&mut record_file).unwrap();

        let paths: Vec<String> = records.into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            vec![
                "foo/__init__.py".to_string(),
                "foo-1.0.0.dist-info/RECORD".to_string()
            ]
        );
    }

    #[test]
    fn test_filter_record_with_conda_packages_end_to_end() {
        let json = r#"{
  "name": "pystac",
  "version": "1.14.3",
  "build": "pyhd8ed1ab_0",
  "build_number": 0,
  "fn": "pystac-1.14.3-pyhd8ed1ab_0.conda",
  "channel": "https://conda.anaconda.org/conda-forge",
  "url": "https://conda.anaconda.org/conda-forge/noarch/pystac-1.14.3-pyhd8ed1ab_0.conda",
  "extracted_package_dir": "/tmp",
  "files": [],
  "paths_data": {
    "paths_version": 1,
    "paths": [
      {
        "_path": "lib/python3.12/site-packages/pystac/__init__.py",
        "path_type": "hardlink"
      },
      {
        "_path": "lib/python3.12/site-packages/pystac/item.py",
        "path_type": "hardlink"
      }
    ]
  },
  "link": {
    "source": "/tmp",
    "type": 1
  }
}"#;
        let prefix_record: rattler_conda_types::PrefixRecord = serde_json::from_str(json).unwrap();

        let temp = tempfile::tempdir().unwrap();
        let prefix = temp.path().join("env");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let dist_info = site_packages.join("pystac-1.15.2.dist-info");
        fs_err::create_dir_all(&dist_info).unwrap();

        let record_path = dist_info.join("RECORD");
        let initial_record = "\
pystac/__init__.py,sha256=111,10
pystac/item.py,sha256=222,20
pystac-1.15.2.dist-info/METADATA,sha256=333,30
pystac-1.15.2.dist-info/RECORD,,
";
        fs_err::write(&record_path, initial_record).unwrap();

        let registry = super::PypiCondaClobberRegistry::with_conda_packages(&[prefix_record]);
        super::filter_record_conda_paths(&dist_info, &prefix, &registry).unwrap();

        let mut record_file = fs_err::File::open(&record_path).unwrap();
        let records = crate::install_wheel::read_record_file(&mut record_file).unwrap();
        let paths: Vec<String> = records.into_iter().map(|r| r.path).collect();
        assert_eq!(
            paths,
            vec![
                "pystac-1.15.2.dist-info/METADATA".to_string(),
                "pystac-1.15.2.dist-info/RECORD".to_string(),
            ]
        );
    }
}
