use std::collections::BTreeSet;
use std::fmt::Display;
use std::path::Path;

use uv_distribution_types::{InstalledDist, InstalledDistKind};
use uv_install_wheel::{Layout, Uninstall};
use uv_installer::UninstallError;

use crate::conda_pypi_clobber::PypiCondaClobberRegistry;

/// Uninstall a package from the specified Python environment, preserving any files
/// that are claimed by installed Conda packages in the prefix.
pub(crate) async fn uninstall(
    dist: &InstalledDist,
    layout: &Layout,
    prefix_root: &Path,
    clobber_registry: &PypiCondaClobberRegistry,
) -> Result<Uninstall, UninstallError> {
    let dist = dist.clone();
    let layout = layout.clone();
    let prefix_root = prefix_root.to_path_buf();
    let clobber_registry = clobber_registry.clone();

    let uninstall = tokio::task::spawn_blocking(move || match dist.kind {
        InstalledDistKind::Registry(_) | InstalledDistKind::Url(_) => Ok(uninstall_wheel(
            dist.install_path(),
            &dist,
            &layout,
            &prefix_root,
            &clobber_registry,
        )?),
        InstalledDistKind::EggInfoDirectory(_) => Ok(uninstall_egg(
            dist.install_path(),
            &dist,
            &prefix_root,
            &clobber_registry,
        )?),
        InstalledDistKind::LegacyEditable(dist) => {
            if clobber_registry
                .is_claimed_by_conda(&dist.egg_link, &prefix_root)
                .is_some()
            {
                Ok(Uninstall {
                    file_count: 0,
                    dir_count: 0,
                })
            } else {
                Ok(uv_install_wheel::uninstall_legacy_editable(&dist.egg_link)?)
            }
        }
        InstalledDistKind::EggInfoFile(dist) => Err(UninstallError::Distutils(dist)),
    })
    .await??;

    Ok(uninstall)
}

/// Uninstall the wheel represented by the given `.dist-info` directory, skipping any
/// files that are claimed by installed Conda packages.
pub(crate) fn uninstall_wheel(
    dist_info: &Path,
    distribution: impl Display,
    layout: &Layout,
    prefix_root: &Path,
    clobber_registry: &PypiCondaClobberRegistry,
) -> Result<Uninstall, uv_install_wheel::Error> {
    let Some(site_packages) = dist_info.parent() else {
        return Err(uv_install_wheel::Error::BrokenVenv(
            "dist-info directory is not in a site-packages directory".to_string(),
        ));
    };

    // Read the RECORD file.
    let record = {
        let record_path = dist_info.join("RECORD");
        let mut record_file = match fs_err::File::open(&record_path) {
            Ok(record_file) => record_file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(uv_install_wheel::Error::MissingRecord(record_path));
            }
            Err(err) => return Err(err.into()),
        };
        uv_install_wheel::read_record(&mut record_file)?
    };

    let mut file_count = 0usize;
    let mut dir_count = 0usize;

    // Uninstall the files, keeping track of any directories that are left empty.
    let mut visited = BTreeSet::new();
    for entry in &record {
        let path = site_packages.join(&entry.path);

        if !is_path_in_scheme(&entry.path, site_packages, &distribution, layout) {
            continue;
        }

        // Check if this path is claimed by an installed Conda package
        if let Some(conda_pkg) = clobber_registry.is_claimed_by_conda(&path, prefix_root) {
            tracing::debug!(
                "Skipping removal of '{}' during PyPI uninstall: claimed by conda package '{}'",
                path.display(),
                conda_pkg.as_source()
            );
            continue;
        }

        match fs_err::remove_file(&path) {
            Ok(()) => {
                tracing::trace!("Removed file: {}", path.display());
                file_count += 1;
                if let Some(parent) = path.parent() {
                    visited.insert(pixi_path::normalize_std(parent));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => match fs_err::remove_dir_all(&path) {
                Ok(()) => {
                    tracing::trace!("Removed directory: {}", path.display());
                    dir_count += 1;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(err.into()),
            },
        }
    }

    // If any directories were left empty, remove them. Iterate in reverse order such that we visit
    // the deepest directories first.
    clean_empty_directories(
        visited,
        site_packages,
        prefix_root,
        clobber_registry,
        &mut dir_count,
    )?;

    Ok(Uninstall {
        file_count,
        dir_count,
    })
}

fn clean_empty_directories(
    visited: BTreeSet<std::path::PathBuf>,
    site_packages: &Path,
    prefix_root: &Path,
    clobber_registry: &PypiCondaClobberRegistry,
    dir_count: &mut usize,
) -> Result<(), uv_install_wheel::Error> {
    for path in visited.iter().rev() {
        // No need to look at directories outside of `site-packages` (like `bin`).
        if !path.starts_with(site_packages) {
            continue;
        }

        let mut path = path.as_path();
        loop {
            // If we reach the site-packages directory, we're done.
            if path == site_packages {
                break;
            }

            // If the directory contains a `__pycache__` directory:
            let pycache = path.join("__pycache__");
            if pycache.is_dir() {
                // Delete pycache entries that are NOT claimed by conda
                if let Ok(read_dir) = fs_err::read_dir(&pycache) {
                    for entry in read_dir.flatten() {
                        let entry_path = entry.path();
                        if clobber_registry
                            .is_claimed_by_conda(&entry_path, prefix_root)
                            .is_none()
                        {
                            let _ = fs_err::remove_file(&entry_path);
                        }
                    }
                }
                // If __pycache__ is now empty, remove it
                if fs_err::read_dir(&pycache).is_ok_and(|mut read_dir| read_dir.next().is_none()) {
                    let _ = fs_err::remove_dir(&pycache);
                    *dir_count += 1;
                }
            }

            // Try to read from the directory. If it doesn't exist, assume we deleted it in a
            // previous iteration.
            let mut read_dir = match fs_err::read_dir(path) {
                Ok(read_dir) => read_dir,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => break,
                Err(err) => return Err(err.into()),
            };

            // If the directory is not empty, we cannot remove it.
            if read_dir.next().is_some() {
                break;
            }

            fs_err::remove_dir(path)?;
            tracing::trace!("Removed directory: {}", path.display());
            *dir_count += 1;

            if let Some(parent) = path.parent() {
                path = parent;
            } else {
                break;
            }
        }
    }
    Ok(())
}

fn is_path_in_scheme(
    path: &str,
    site_packages: &Path,
    distribution: impl Display,
    layout: &Layout,
) -> bool {
    let normalized = pixi_path::normalize_std(&site_packages.join(path));

    if normalized.starts_with(&layout.scheme.data)
        || normalized.starts_with(&layout.scheme.purelib)
        || normalized.starts_with(&layout.scheme.platlib)
        || normalized.starts_with(&layout.scheme.scripts)
        || normalized.starts_with(&layout.scheme.include)
    {
        true
    } else {
        tracing::warn!(
            "Invalid RECORD entry in {} that escapes the Python environment, skipping: {}",
            distribution,
            path
        );
        false
    }
}

fn is_safe_top_level_entry(entry: &str) -> bool {
    !entry.is_empty() && entry != "." && entry != ".." && !entry.contains(['/', '\\'])
}

pub(crate) fn uninstall_egg(
    egg_info: &Path,
    distribution: impl Display,
    prefix_root: &Path,
    clobber_registry: &PypiCondaClobberRegistry,
) -> Result<Uninstall, uv_install_wheel::Error> {
    let mut file_count = 0usize;
    let mut dir_count = 0usize;

    let Some(dist_location) = egg_info.parent() else {
        return Err(uv_install_wheel::Error::BrokenVenv(
            "egg-info directory is not in a site-packages directory".to_string(),
        ));
    };

    let namespace_packages = {
        let namespace_packages_path = egg_info.join("namespace_packages.txt");
        match fs_err::read_to_string(namespace_packages_path) {
            Ok(namespace_packages) => namespace_packages
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => vec![],
            Err(err) => return Err(err.into()),
        }
    };

    let top_level = {
        let top_level_path = egg_info.join("top_level.txt");
        match fs_err::read_to_string(&top_level_path) {
            Ok(top_level) => top_level
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .filter(|line| !namespace_packages.iter().any(|ns| ns.as_str() == *line))
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(uv_install_wheel::Error::MissingTopLevel(top_level_path));
            }
            Err(err) => return Err(err.into()),
        }
    };

    for entry in top_level {
        if !is_safe_top_level_entry(&entry) {
            tracing::warn!(
                "Invalid top_level.txt entry in {} that is not a top-level module or package, skipping: {}",
                distribution,
                entry
            );
            continue;
        }

        let path = dist_location.join(&entry);

        if path.is_dir() {
            if clobber_registry.has_claimed_paths_under(&path, prefix_root) {
                remove_dir_contents_preserving_conda(
                    &path,
                    prefix_root,
                    clobber_registry,
                    &mut file_count,
                    &mut dir_count,
                )?;
            } else {
                match fs_err::remove_dir_all(&path) {
                    Ok(()) => {
                        dir_count += 1;
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err.into()),
                }
            }
            continue;
        }

        for extension in &["py", "pyc", "pyo"] {
            let path = path.with_extension(extension);
            if clobber_registry
                .is_claimed_by_conda(&path, prefix_root)
                .is_some()
            {
                continue;
            }
            match fs_err::remove_file(&path) {
                Ok(()) => {
                    file_count += 1;
                    break;
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    // Remove the .egg-info directory itself if not claimed by conda
    if clobber_registry
        .is_claimed_by_conda(egg_info, prefix_root)
        .is_none()
    {
        match fs_err::remove_dir_all(egg_info) {
            Ok(()) => {
                dir_count += 1;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }

    Ok(Uninstall {
        file_count,
        dir_count,
    })
}

fn remove_dir_contents_preserving_conda(
    dir: &Path,
    prefix_root: &Path,
    clobber_registry: &PypiCondaClobberRegistry,
    file_count: &mut usize,
    dir_count: &mut usize,
) -> Result<(), uv_install_wheel::Error> {
    let read_dir = match fs_err::read_dir(dir) {
        Ok(read_dir) => read_dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };

    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_dir_contents_preserving_conda(
                &path,
                prefix_root,
                clobber_registry,
                file_count,
                dir_count,
            )?;
        } else if clobber_registry
            .is_claimed_by_conda(&path, prefix_root)
            .is_none()
            && fs_err::remove_file(&path).is_ok()
        {
            *file_count += 1;
        }
    }

    // If dir is now empty, remove it
    if fs_err::read_dir(dir).is_ok_and(|mut read_dir| read_dir.next().is_none()) {
        let _ = fs_err::remove_dir(dir);
        *dir_count += 1;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::PrefixRecord;

    fn make_test_layout(prefix_root: &Path) -> Layout {
        Layout {
            sys_executable: prefix_root.join("bin/python"),
            python_version: (3, 12),
            os_name: "posix".to_string(),
            scheme: uv_pypi_types::Scheme {
                purelib: prefix_root.join("lib/python3.12/site-packages"),
                platlib: prefix_root.join("lib/python3.12/site-packages"),
                scripts: prefix_root.join("bin"),
                data: prefix_root.to_path_buf(),
                include: prefix_root.join("include"),
            },
        }
    }

    #[test]
    fn test_uninstall_wheel_preserves_conda_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix_root = temp_dir.path();
        let layout = make_test_layout(prefix_root);
        let site_packages = prefix_root.join("lib/python3.12/site-packages");
        let pkg_dir = site_packages.join("pystac");
        fs_err::create_dir_all(&pkg_dir).unwrap();

        // Create package files
        let conda_file = pkg_dir.join("__init__.py");
        let pypi_only_file = pkg_dir.join("extra.py");
        fs_err::write(&conda_file, b"# conda file").unwrap();
        fs_err::write(&pypi_only_file, b"# pypi only file").unwrap();

        // Create dist-info
        let dist_info = site_packages.join("pystac_core-1.15.2.dist-info");
        fs_err::create_dir_all(&dist_info).unwrap();
        let record_content = "pystac/__init__.py,sha256=xxx,12\npystac/extra.py,sha256=yyy,17\npystac_core-1.15.2.dist-info/RECORD,,\n";
        fs_err::write(dist_info.join("RECORD"), record_content).unwrap();

        // Create conda clobber registry claiming __init__.py
        let prefix_record_json = r#"{
          "name": "pystac",
          "version": "1.14.3",
          "build": "0",
          "build_number": 0,
          "subdir": "noarch",
          "fn": "pystac-1.14.3-0.conda",
          "url": "https://conda.anaconda.org/conda-forge/noarch/pystac-1.14.3-0.conda",
          "channel": "https://conda.anaconda.org/conda-forge",
          "extracted_package_dir": "",
          "files": [
            "lib/python3.12/site-packages/pystac/core.py"
          ],
          "paths_data": {
            "paths_version": 1,
            "paths": [
              {
                "_path": "lib/python3.12/site-packages/pystac/__init__.py",
                "path_type": "hardlink"
              }
            ]
          }
        }"#;
        let prefix_record: PrefixRecord = serde_json::from_str(prefix_record_json).unwrap();
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[prefix_record]);
        let summary =
            uninstall_wheel(&dist_info, "pystac-core", &layout, prefix_root, &registry).unwrap();

        // Conda file must be preserved!
        assert!(conda_file.exists(), "conda file was unexpectedly deleted");
        // PyPI-only file must be removed!
        assert!(!pypi_only_file.exists(), "pypi only file was not deleted");
        // Dist-info directory must be removed!
        assert!(!dist_info.exists(), "dist-info was not deleted");
        // pystac directory must still exist because it contains the conda file!
        assert!(
            pkg_dir.exists(),
            "package directory was unexpectedly deleted"
        );
        assert!(summary.file_count >= 2); // extra.py and RECORD
    }

    #[test]
    fn test_uninstall_wheel_cleans_up_empty_directories_when_no_conda_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix_root = temp_dir.path();
        let layout = make_test_layout(prefix_root);
        let site_packages = prefix_root.join("lib/python3.12/site-packages");
        let pkg_dir = site_packages.join("unrelated_pkg");
        fs_err::create_dir_all(&pkg_dir).unwrap();

        let file = pkg_dir.join("__init__.py");
        fs_err::write(&file, b"# code").unwrap();

        let dist_info = site_packages.join("unrelated_pkg-1.0.0.dist-info");
        fs_err::create_dir_all(&dist_info).unwrap();
        let record_content =
            "unrelated_pkg/__init__.py,sha256=xxx,6\nunrelated_pkg-1.0.0.dist-info/RECORD,,\n";
        fs_err::write(dist_info.join("RECORD"), record_content).unwrap();

        let registry = PypiCondaClobberRegistry::default();
        let summary =
            uninstall_wheel(&dist_info, "unrelated_pkg", &layout, prefix_root, &registry).unwrap();

        assert!(!file.exists(), "file should be deleted");
        assert!(!pkg_dir.exists(), "pkg_dir should be deleted when empty");
        assert!(!dist_info.exists(), "dist_info should be deleted");
        assert_eq!(summary.file_count, 2);
    }

    #[test]
    fn test_uninstall_wheel_pycache_preserves_conda_pyc() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix_root = temp_dir.path();
        let layout = make_test_layout(prefix_root);
        let site_packages = prefix_root.join("lib/python3.12/site-packages");
        let pkg_dir = site_packages.join("cached_pkg");
        let pycache_dir = pkg_dir.join("__pycache__");
        fs_err::create_dir_all(&pycache_dir).unwrap();

        let conda_pyc = pycache_dir.join("module.cpython-312.pyc");
        let pypi_pyc = pycache_dir.join("other.cpython-312.pyc");
        fs_err::write(&conda_pyc, b"conda pyc").unwrap();
        fs_err::write(&pypi_pyc, b"pypi pyc").unwrap();

        let pypi_file = pkg_dir.join("other.py");
        fs_err::write(&pypi_file, b"other py").unwrap();

        let dist_info = site_packages.join("cached_pkg-1.0.0.dist-info");
        fs_err::create_dir_all(&dist_info).unwrap();
        let record_content =
            "cached_pkg/other.py,sha256=xxx,8\ncached_pkg-1.0.0.dist-info/RECORD,,\n";
        fs_err::write(dist_info.join("RECORD"), record_content).unwrap();

        let prefix_record_json = r#"{
          "name": "cached_pkg",
          "version": "1.0.0",
          "build": "0",
          "build_number": 0,
          "subdir": "noarch",
          "fn": "cached_pkg-1.0.0-0.conda",
          "url": "https://conda.anaconda.org/conda-forge/noarch/cached_pkg-1.0.0-0.conda",
          "channel": "https://conda.anaconda.org/conda-forge",
          "extracted_package_dir": "",
          "files": [
            "lib/python3.12/site-packages/cached_pkg/__pycache__/module.cpython-312.pyc"
          ],
          "paths_data": {
            "paths_version": 1,
            "paths": []
          }
        }"#;
        let prefix_record: PrefixRecord = serde_json::from_str(prefix_record_json).unwrap();
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[prefix_record]);

        uninstall_wheel(&dist_info, "cached_pkg", &layout, prefix_root, &registry).unwrap();

        assert!(conda_pyc.exists(), "conda pyc should be preserved");
        assert!(!pypi_pyc.exists(), "pypi pyc should be deleted");
        assert!(!pypi_file.exists(), "pypi source file should be deleted");
        assert!(
            pycache_dir.exists(),
            "pycache directory should remain since conda pyc is inside"
        );
    }

    #[test]
    fn test_uninstall_egg_preserves_conda_files() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix_root = temp_dir.path();
        let site_packages = prefix_root.join("lib/python3.12/site-packages");
        let pkg_dir = site_packages.join("egg_pkg");
        fs_err::create_dir_all(&pkg_dir).unwrap();

        let conda_file = pkg_dir.join("conda_mod.py");
        let pypi_file = pkg_dir.join("pypi_mod.py");
        fs_err::write(&conda_file, b"# conda").unwrap();
        fs_err::write(&pypi_file, b"# pypi").unwrap();

        let egg_info = site_packages.join("egg_pkg-1.0.0.egg-info");
        fs_err::create_dir_all(&egg_info).unwrap();
        fs_err::write(egg_info.join("top_level.txt"), "egg_pkg\n").unwrap();

        let prefix_record_json = r#"{
          "name": "egg_pkg",
          "version": "1.0.0",
          "build": "0",
          "build_number": 0,
          "subdir": "noarch",
          "fn": "egg_pkg-1.0.0-0.conda",
          "url": "https://conda.anaconda.org/conda-forge/noarch/egg_pkg-1.0.0-0.conda",
          "channel": "https://conda.anaconda.org/conda-forge",
          "extracted_package_dir": "",
          "files": [
            "lib/python3.12/site-packages/egg_pkg/conda_mod.py"
          ],
          "paths_data": {
            "paths_version": 1,
            "paths": []
          }
        }"#;
        let prefix_record: PrefixRecord = serde_json::from_str(prefix_record_json).unwrap();
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[prefix_record]);

        uninstall_egg(&egg_info, "egg_pkg", prefix_root, &registry).unwrap();

        assert!(conda_file.exists(), "conda file in egg must be preserved");
        assert!(!pypi_file.exists(), "pypi file in egg must be deleted");
        assert!(!egg_info.exists(), "egg-info directory must be deleted");
        assert!(
            pkg_dir.exists(),
            "package directory containing conda file must remain"
        );
    }
}
