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

use super::install_wheel::get_wheel_info;

const MAX_CLOBBER_PATHS_PER_PACKAGE: usize = 5;

#[derive(Default, Debug)]
pub(crate) struct ClobberReport(BTreeMap<(String, String), Vec<PathBuf>>);

impl ClobberReport {
    fn entry(
        &mut self,
        key: (String, String),
    ) -> btree_map::Entry<'_, (String, String), Vec<PathBuf>> {
        self.0.entry(key)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn keys(&self) -> btree_map::Keys<'_, (String, String), Vec<PathBuf>> {
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
                writeln!(f, "    - {}", path.display())?;
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
    paths_registry: AHashMap<PathBuf, rattler_conda_types::PackageName>,
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

struct WheelInstallPaths<'a> {
    root_scheme: &'a Path,
    purelib: &'a Path,
    platlib: &'a Path,
    headers: &'a Path,
    scripts: &'a Path,
    data: &'a Path,
}

fn wheel_record_install_path(
    install_paths: &WheelInstallPaths<'_>,
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

    install_paths.root_scheme.join(record_path)
}

fn conda_relative_wheel_record_path(
    install_paths: &WheelInstallPaths<'_>,
    record_path: impl AsRef<Path>,
    prefix_root: &Path,
) -> Option<PathBuf> {
    normalize_std(&wheel_record_install_path(install_paths, record_path))
        .strip_prefix(prefix_root)
        .ok()
        .map(Path::to_path_buf)
}

impl PypiCondaClobberRegistry {
    /// Register the paths of the installed conda packages
    /// to later check if they are going to be clobbered by the installation of the wheels
    pub(crate) fn with_conda_packages(conda_packages: &[PrefixRecord]) -> Self {
        let mut registry = AHashMap::with_capacity(conda_packages.len() * 50);
        for record in conda_packages {
            for path in &record.paths_data.paths {
                registry.insert(
                    path.relative_path.clone(),
                    record.repodata_record.package_record.name.clone(),
                );
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
        prefix_root: &Path,
    ) -> miette::Result<Option<ClobberReport>> {
        let mut clobber_report = ClobberReport::default();

        for wheel in wheels {
            let pypi_package = wheel.name().to_string();
            let Ok(Some(whl_info)) = get_wheel_info(wheel.path(), venv) else {
                continue;
            };

            let install_paths = WheelInstallPaths {
                root_scheme: &whl_info.1,
                purelib: venv.interpreter().purelib(),
                platlib: venv.interpreter().platlib(),
                headers: venv.interpreter().include(),
                scripts: venv.scripts(),
                data: venv.interpreter().data(),
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
            for entry in whl_info.0 {
                let Some(path_to_clobber) =
                    conda_relative_wheel_record_path(&install_paths, entry.path, prefix_root)
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
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{
        ClobberReport, WheelDataScheme, WheelInstallPaths, conda_relative_wheel_record_path,
        parse_wheel_data_path,
    };

    fn install_paths(prefix: &Path) -> WheelInstallPaths<'_> {
        WheelInstallPaths {
            root_scheme: Path::new("/prefix/lib/python3.12/site-packages"),
            purelib: Path::new("/prefix/lib/python3.12/site-packages"),
            platlib: Path::new("/prefix/lib/python3.12/site-packages"),
            headers: Path::new("/prefix/include/python3.12"),
            scripts: Path::new("/prefix/bin"),
            data: prefix,
        }
    }

    #[test]
    fn record_path_escaping_site_packages_is_matched_prefix_relative() {
        let prefix = Path::new("/prefix");
        let install_paths = install_paths(prefix);

        assert_eq!(
            conda_relative_wheel_record_path(&install_paths, "../../../bin/prek", prefix),
            Some(PathBuf::from("bin/prek"))
        );
    }

    #[test]
    fn record_path_outside_prefix_is_ignored() {
        let prefix = Path::new("/prefix");
        let install_paths = install_paths(prefix);

        assert_eq!(
            conda_relative_wheel_record_path(&install_paths, "../../../../../bin/prek", prefix),
            None
        );
    }

    #[test]
    fn parses_pep427_data_scheme_paths() {
        assert_eq!(
            parse_wheel_data_path(Path::new("prek-0.4.4.data/scripts/prek")),
            Some((WheelDataScheme::Scripts, Path::new("prek")))
        );
        assert_eq!(
            parse_wheel_data_path(Path::new("pkg-1.0.data/purelib/module.py")),
            Some((WheelDataScheme::Purelib, Path::new("module.py")))
        );
        assert_eq!(parse_wheel_data_path(Path::new("prek/__init__.py")), None);
    }

    #[test]
    fn wheel_data_scripts_path_is_matched_prefix_relative() {
        let prefix = Path::new("/prefix");
        let install_paths = install_paths(prefix);

        assert_eq!(
            conda_relative_wheel_record_path(
                &install_paths,
                "prek-0.4.4.data/scripts/prek",
                prefix
            ),
            Some(PathBuf::from("bin/prek"))
        );
    }

    #[test]
    fn wheel_data_scheme_paths_are_matched_prefix_relative() {
        let prefix = Path::new("/prefix");
        let install_paths = install_paths(prefix);

        assert_eq!(
            conda_relative_wheel_record_path(
                &install_paths,
                "pkg-1.0.data/purelib/module.py",
                prefix
            ),
            Some(PathBuf::from("lib/python3.12/site-packages/module.py"))
        );
        assert_eq!(
            conda_relative_wheel_record_path(
                &install_paths,
                "pkg-1.0.data/platlib/native.so",
                prefix
            ),
            Some(PathBuf::from("lib/python3.12/site-packages/native.so"))
        );
        assert_eq!(
            conda_relative_wheel_record_path(&install_paths, "pkg-1.0.data/headers/pkg.h", prefix),
            Some(PathBuf::from("include/python3.12/pkg.h"))
        );
        assert_eq!(
            conda_relative_wheel_record_path(
                &install_paths,
                "pkg-1.0.data/data/share/pkg/data.txt",
                prefix
            ),
            Some(PathBuf::from("share/pkg/data.txt"))
        );
    }

    #[test]
    fn clobber_warning_groups_by_package_and_limits_files() {
        let mut report = ClobberReport::default();
        report
            .entry(("prek".to_string(), "prek".to_string()))
            .or_default()
            .extend((1..=7).map(|idx| PathBuf::from(format!("bin/prek-{idx}"))));

        assert_eq!(
            report.to_string(),
            "PyPI package files will overwrite files installed by conda packages:\n  - PyPI package 'prek' overwrites conda package 'prek':\n    - bin/prek-1\n    - bin/prek-2\n    - bin/prek-3\n    - bin/prek-4\n    - bin/prek-5\n    - ... 2 other files\n"
        );
    }
}
