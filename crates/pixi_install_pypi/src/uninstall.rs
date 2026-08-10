use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use pixi_path::normalize_std;
use uv_distribution_types::{InstalledDist, InstalledDistKind};
use uv_install_wheel::{Layout, RecordEntry, Uninstall};
use uv_installer::UninstallError;

use crate::conda_pypi_clobber::{
    CondaPathState, PypiCondaClobberRegistry, filesystem_relative_path,
    recursive_removal_relative_path,
};

static TEMP_DIST_INFO_COUNTER: AtomicU64 = AtomicU64::new(0);
const TEMP_DIST_INFO_ATTEMPTS: usize = 100;

/// A temporary metadata directory next to the installed distribution.
///
/// uv resolves RECORD paths relative to the metadata directory's parent, so
/// the filtered RECORD must remain inside the original site-packages directory.
struct TemporaryDistInfo {
    path: PathBuf,
}

impl TemporaryDistInfo {
    fn new(site_packages: &Path) -> Result<Self, uv_install_wheel::Error> {
        for _ in 0..TEMP_DIST_INFO_ATTEMPTS {
            let id = TEMP_DIST_INFO_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = site_packages.join(format!(".pixi-uninstall-{}-{id}", std::process::id()));
            match fs_err::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err.into()),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "failed to create a unique temporary dist-info directory",
        )
        .into())
    }

    fn write_record(
        &self,
        records: impl IntoIterator<Item = RecordEntry>,
    ) -> Result<(), uv_install_wheel::Error> {
        let mut writer = csv::WriterBuilder::new()
            .has_headers(false)
            .escape(b'"')
            .from_path(self.path.join("RECORD"))?;
        for record in records {
            writer.serialize(record)?;
        }
        writer.flush()?;
        Ok(())
    }
}

impl Drop for TemporaryDistInfo {
    fn drop(&mut self) {
        if let Err(err) = fs_err::remove_dir_all(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::debug!(
                "failed to remove temporary dist-info directory {}: {err}",
                self.path.display()
            );
        }
    }
}

fn clean_unowned_pycache_entries(
    pycache: &Path,
    protected_entries: &ahash::AHashSet<PathBuf>,
) -> Result<Uninstall, uv_install_wheel::Error> {
    fn clean_directory(
        directory: &Path,
        relative_directory: &Path,
        protected_entries: &ahash::AHashSet<PathBuf>,
        summary: &mut Uninstall,
    ) -> Result<(), uv_install_wheel::Error> {
        let entries = match fs_err::read_dir(directory) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(err.into()),
        };
        for entry in entries {
            let entry = entry?;
            let relative_path = relative_directory.join(entry.file_name());
            let installed_path = entry.path();
            let file_type = entry.file_type()?;
            let contains_protected_path = protected_entries.iter().try_fold(
                false,
                |contains_protected_path, protected_path| {
                    if contains_protected_path {
                        Ok(true)
                    } else {
                        filesystem_relative_path(&installed_path, protected_path)
                            .map(|relative| relative.is_some())
                    }
                },
            )?;
            if file_type.is_dir() && !file_type.is_symlink() {
                if contains_protected_path {
                    clean_directory(&installed_path, &relative_path, protected_entries, summary)?;
                } else {
                    fs_err::remove_dir_all(installed_path)?;
                    summary.dir_count += 1;
                }
            } else if !contains_protected_path {
                fs_err::remove_file(installed_path)?;
                summary.file_count += 1;
            }
        }
        Ok(())
    }

    let metadata = match fs_err::symlink_metadata(pycache) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Uninstall::default()),
        Err(err) => return Err(err.into()),
    };
    if !metadata.file_type().is_dir() {
        return Ok(Uninstall::default());
    }

    let mut summary = Uninstall::default();
    clean_directory(pycache, Path::new(""), protected_entries, &mut summary)?;
    Ok(summary)
}

fn prune_empty_parent_directories(
    prefix: &Path,
    site_packages: &Path,
    conda_registry: &PypiCondaClobberRegistry,
    parents: &mut Vec<PathBuf>,
    summary: &mut Uninstall,
) -> Result<(), uv_install_wheel::Error> {
    parents.sort_unstable_by(|left, right| {
        right
            .components()
            .count()
            .cmp(&left.components().count())
            .then_with(|| left.cmp(right))
    });
    parents.dedup();

    for parent in parents {
        let mut directory = parent.as_path();
        while let Some(relative_directory) = filesystem_relative_path(site_packages, directory)? {
            if relative_directory.as_os_str().is_empty() {
                break;
            }
            let metadata = match fs_err::symlink_metadata(directory) {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == io::ErrorKind::NotFound => break,
                Err(err) => return Err(err.into()),
            };
            if !metadata.file_type().is_dir()
                || conda_registry.current_directory_is_conda_owned(prefix, directory)?
            {
                break;
            }
            let mut entries = match fs_err::read_dir(directory) {
                Ok(entries) => entries,
                Err(err) if err.kind() == io::ErrorKind::NotFound => break,
                Err(err) => return Err(err.into()),
            };
            if entries.next().is_some() {
                break;
            }

            fs_err::remove_dir(directory)?;
            summary.dir_count += 1;
            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent;
        }
    }
    Ok(())
}

pub(crate) fn remove_tree_preserving_conda_paths(
    path: &Path,
    prefix: &Path,
    conda_registry: &PypiCondaClobberRegistry,
) -> Result<Uninstall, uv_install_wheel::Error> {
    fn remove_path(
        path: &Path,
        prefix: &Path,
        conda_registry: &PypiCondaClobberRegistry,
        summary: &mut Uninstall,
    ) -> Result<bool, uv_install_wheel::Error> {
        recursive_removal_relative_path(prefix, path)?;
        let metadata = match fs_err::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => return Err(err.into()),
        };

        let preserve_directory = match conda_registry.installed_path_state(prefix, path)? {
            CondaPathState::Owned if metadata.file_type().is_dir() => true,
            CondaPathState::Owned => return Ok(true),
            CondaPathState::Clobbered(package) => {
                return Err(uv_install_wheel::Error::InvalidWheel(format!(
                    "refusing to remove {} before Conda package '{}' is relinked",
                    path.display(),
                    package.as_normalized()
                )));
            }
            CondaPathState::Untracked => false,
        };

        if metadata.file_type().is_dir() {
            for entry in fs_err::read_dir(path)? {
                let entry = entry?;
                remove_path(&entry.path(), prefix, conda_registry, summary)?;
            }
            if !preserve_directory && fs_err::read_dir(path)?.next().is_none() {
                fs_err::remove_dir(path)?;
                summary.dir_count += 1;
                return Ok(false);
            }
            Ok(true)
        } else {
            fs_err::remove_file(path)?;
            summary.file_count += 1;
            Ok(false)
        }
    }

    recursive_removal_relative_path(prefix, path)?;
    let clobbered_packages = conda_registry.packages_requiring_reinstall_for_tree(prefix, path)?;
    if !clobbered_packages.is_empty() {
        let mut packages = clobbered_packages
            .iter()
            .map(|package| package.as_normalized())
            .collect::<Vec<_>>();
        packages.sort_unstable();
        return Err(uv_install_wheel::Error::InvalidWheel(format!(
            "refusing to remove {} before Conda packages are relinked: {}",
            path.display(),
            packages.join(", ")
        )));
    }

    let mut summary = Uninstall::default();
    remove_path(path, prefix, conda_registry, &mut summary)?;
    Ok(summary)
}

pub(crate) fn ensure_conda_safe_uninstall_supported(
    dist: &InstalledDist,
) -> Result<(), uv_install_wheel::Error> {
    if matches!(
        &dist.kind,
        InstalledDistKind::Registry(_) | InstalledDistKind::Url(_)
    ) {
        return Ok(());
    }

    Err(uv_install_wheel::Error::InvalidWheel(format!(
        "refusing to uninstall legacy Python distribution {dist}: its metadata cannot be reconciled safely with Conda path ownership"
    )))
}

/// Uninstall a distribution without deleting wheel files owned by currently
/// installed conda packages.
pub(crate) async fn uninstall_preserving_conda_paths(
    dist: &InstalledDist,
    layout: &Layout,
    prefix: &Path,
    conda_registry: Arc<PypiCondaClobberRegistry>,
) -> Result<Uninstall, UninstallError> {
    ensure_conda_safe_uninstall_supported(dist)?;

    let dist = dist.clone();
    let layout = layout.clone();
    let prefix = prefix.to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<Uninstall, uv_install_wheel::Error> {
        let record_path = dist.install_path().join("RECORD");
        let record_file = match fs_err::File::open(&record_path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(uv_install_wheel::Error::MissingRecord(record_path));
            }
            Err(err) => return Err(err.into()),
        };
        let records = uv_install_wheel::read_record(record_file)?;
        let site_packages = dist.install_path().parent().ok_or_else(|| {
            uv_install_wheel::Error::BrokenVenv(
                "dist-info directory is not in a site-packages directory".to_string(),
            )
        })?;
        let protection = conda_registry.conda_owned_record_paths(
            &prefix,
            site_packages,
            records.iter(),
        )?;

        if protection.owned.is_empty()
            && protection.unsafe_to_remove.is_empty()
            && protection.cleanup_sensitive.is_empty()
        {
            return uv_install_wheel::uninstall_wheel(dist.install_path(), &dist, &layout);
        }

        tracing::debug!(
            "Preserving {} conda-owned and {} unsafe RECORD path(s) while uninstalling {}",
            protection.owned.len(),
            protection.unsafe_to_remove.len(),
            dist
        );

        let mut filtered_records = Vec::with_capacity(records.len());
        let mut cleanup_sensitive_paths = Vec::new();
        for record in records {
            if protection.owned.contains(&record.path)
                || protection.unsafe_to_remove.contains(&record.path)
            {
                continue;
            }
            if protection.cleanup_sensitive.contains(&record.path) {
                cleanup_sensitive_paths.push(record.path);
            } else {
                filtered_records.push(record);
            }
        }

        let temporary_dist_info = TemporaryDistInfo::new(site_packages)?;
        temporary_dist_info.write_record(filtered_records)?;

        let mut summary =
            uv_install_wheel::uninstall_wheel(&temporary_dist_info.path, &dist, &layout)?;
        let normalized_site_packages = normalize_std(site_packages);
        let mut cleanup_parents = Vec::new();
        for record_path in cleanup_sensitive_paths {
            let path = normalize_std(&site_packages.join(record_path));
            match fs_err::remove_file(&path) {
                Ok(()) => {
                    summary.file_count += 1;
                    if let Some(parent) = path.parent()
                        && filesystem_relative_path(&normalized_site_packages, parent)?.is_some()
                    {
                        cleanup_parents.push(parent.to_path_buf());
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(err) if path.is_dir() => {
                    return Err(uv_install_wheel::Error::InvalidWheel(format!(
                        "RECORD entry points to directory {} that cannot be removed without conda cleanup safeguards: {err}",
                        path.display()
                    )));
                }
                Err(err) => return Err(err.into()),
            }
        }

        for (directory, protected_entries) in protection.protected_pycache_paths {
            let pycache = prefix.join(directory).join("__pycache__");
            let cleanup = clean_unowned_pycache_entries(&pycache, &protected_entries)?;
            summary.file_count += cleanup.file_count;
            summary.dir_count += cleanup.dir_count;
        }

        prune_empty_parent_directories(
            &prefix,
            &normalized_site_packages,
            &conda_registry,
            &mut cleanup_parents,
            &mut summary,
        )?;

        Ok(summary)
    })
    .await?
    .map_err(UninstallError::from)
}

pub(crate) async fn uninstall_or_remove_metadata_preserving_conda_paths(
    dist: &InstalledDist,
    layout: &Layout,
    prefix: &Path,
    conda_registry: Arc<PypiCondaClobberRegistry>,
) -> Result<Uninstall, UninstallError> {
    match uninstall_preserving_conda_paths(dist, layout, prefix, Arc::clone(&conda_registry)).await
    {
        Ok(summary) => Ok(summary),
        Err(UninstallError::Uninstall(error))
            if matches!(error, uv_install_wheel::Error::MissingRecord(_)) =>
        {
            tracing::debug!("Uninstall failed for {dist:?} with error: {error}");
            remove_tree_preserving_conda_paths(dist.install_path(), prefix, &conda_registry)
                .map_err(UninstallError::from)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr, sync::Arc};

    use ahash::AHashSet;
    use rattler_conda_types::{
        PackageName, PackageRecord, PrefixRecord, RepoDataRecord, Version,
        package::{CondaArchiveType, DistArchiveIdentifier},
        prefix_record::{PathType, PathsEntry},
    };
    use url::Url;
    use uv_distribution_types::{
        InstalledDist, InstalledDistKind, InstalledEggInfoDirectory, InstalledRegistryDist,
    };
    use uv_install_wheel::{Layout, RecordEntry};

    use super::{
        clean_unowned_pycache_entries, uninstall_or_remove_metadata_preserving_conda_paths,
        uninstall_preserving_conda_paths,
    };
    use crate::conda_pypi_clobber::PypiCondaClobberRegistry;

    fn installed_dist(path: PathBuf) -> InstalledDist {
        InstalledDist::from(InstalledDistKind::Registry(InstalledRegistryDist {
            name: uv_normalize::PackageName::from_str("example").unwrap(),
            version: uv_pep440::Version::from_str("1.0").unwrap(),
            path: path.into(),
            cache_info: None,
            build_info: None,
        }))
    }

    fn installed_egg_info_directory(path: PathBuf) -> InstalledDist {
        InstalledDist::from(InstalledDistKind::EggInfoDirectory(
            InstalledEggInfoDirectory {
                name: uv_normalize::PackageName::from_str("example").unwrap(),
                version: uv_pep440::Version::from_str("1.0").unwrap(),
                path: path.into(),
            },
        ))
    }

    fn layout(prefix: &std::path::Path, site_packages: &std::path::Path) -> Layout {
        Layout {
            sys_executable: prefix.join("bin/python"),
            python_version: (3, 12),
            os_name: std::env::consts::OS.to_string(),
            scheme: uv_pypi_types::Scheme {
                purelib: site_packages.to_path_buf(),
                platlib: site_packages.to_path_buf(),
                scripts: prefix.join("bin"),
                data: prefix.to_path_buf(),
                include: prefix.join("include"),
            },
        }
    }

    #[cfg(unix)]
    fn conda_softlink_record(path: PathBuf, target: &[u8]) -> PrefixRecord {
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
            vec![PathsEntry {
                relative_path: path,
                original_path: None,
                path_type: PathType::SoftLink,
                no_link: false,
                sha256: Some(
                    rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(target),
                ),
                sha256_in_prefix: None,
                size_in_bytes: None,
                file_mode: None,
                prefix_placeholder: None,
            }],
        )
    }

    fn conda_file_record(path: PathBuf, contents: &[u8]) -> PrefixRecord {
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
            vec![PathsEntry {
                relative_path: path,
                original_path: None,
                path_type: PathType::HardLink,
                no_link: false,
                sha256: Some(
                    rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(contents),
                ),
                sha256_in_prefix: None,
                size_in_bytes: Some(contents.len() as u64),
                file_mode: None,
                prefix_placeholder: None,
            }],
        )
    }

    fn conda_directory_record(path: PathBuf) -> PrefixRecord {
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
            vec![PathsEntry {
                relative_path: path,
                original_path: None,
                path_type: PathType::Directory,
                no_link: false,
                sha256: None,
                sha256_in_prefix: None,
                size_in_bytes: None,
                file_mode: None,
                prefix_placeholder: None,
            }],
        )
    }

    #[tokio::test]
    async fn uninstall_preserves_directory_record_with_conda_owned_descendant() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let package = site_packages.join("pkg");
        let dist_info = site_packages.join("example-1.0.dist-info");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/pkg/module.py");
        let conda_contents = b"conda module";
        fs_err::create_dir_all(&package).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "pkg/,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&prefix, &site_packages);
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_file_record(
            conda_path.clone(),
            conda_contents,
        )]);

        uninstall_preserving_conda_paths(&dist, &layout, &prefix, Arc::new(registry))
            .await
            .unwrap();

        assert!(package.is_dir());
        assert_eq!(
            fs_err::read(prefix.join(conda_path)).unwrap(),
            conda_contents
        );
        assert!(!dist_info.exists());
    }

    #[tokio::test]
    async fn uninstall_removes_directory_record_without_conda_owned_descendant() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let package = site_packages.join("pkg");
        let wheel_directory = package.join("wheel-dir");
        let dist_info = site_packages.join("example-1.0.dist-info");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/pkg/__pycache__/conda.pyc");
        let conda_contents = b"conda bytecode";
        fs_err::create_dir_all(&wheel_directory).unwrap();
        fs_err::create_dir_all(prefix.join(conda_path.parent().unwrap())).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(wheel_directory.join("module.py"), b"wheel module").unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "pkg/wheel-dir/,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&prefix, &site_packages);
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_file_record(
            conda_path.clone(),
            conda_contents,
        )]);

        uninstall_preserving_conda_paths(&dist, &layout, &prefix, Arc::new(registry))
            .await
            .unwrap();

        assert!(!wheel_directory.exists());
        assert_eq!(
            fs_err::read(prefix.join(conda_path)).unwrap(),
            conda_contents
        );
        assert!(!dist_info.exists());
    }

    #[tokio::test]
    async fn uninstall_prunes_empty_parents_after_guarded_file_cleanup() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let package = site_packages.join("pkg");
        let conda_directory = site_packages.join("conda-dir");
        let dist_info = site_packages.join("example-1.0.dist-info");
        fs_err::create_dir_all(&package).unwrap();
        fs_err::create_dir(&conda_directory).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(package.join("module.py"), b"wheel module").unwrap();
        fs_err::write(conda_directory.join("wheel.py"), b"wheel module").unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "pkg/module.py,,\n",
                "conda-dir/wheel.py,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&prefix, &site_packages);
        let site_packages_path = PathBuf::from("lib/python3.12/site-packages");
        let conda_directory_path = site_packages_path.join("conda-dir");
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[
            conda_directory_record(PathBuf::from("lib/python3.12/site-packages")),
            conda_directory_record(conda_directory_path),
        ]);

        uninstall_preserving_conda_paths(&dist, &layout, &prefix, Arc::new(registry))
            .await
            .unwrap();

        assert!(site_packages.is_dir());
        assert!(!package.exists());
        assert!(conda_directory.is_dir());
        assert!(!conda_directory.join("wheel.py").exists());
        assert!(!dist_info.exists());
    }

    #[tokio::test]
    async fn missing_record_cleanup_preserves_conda_owned_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let dist_info = site_packages.join("example-1.0.dist-info");
        let conda_path =
            PathBuf::from("lib/python3.12/site-packages/example-1.0.dist-info/conda-owned.txt");
        let conda_contents = b"conda metadata";
        fs_err::create_dir_all(&dist_info).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        fs_err::write(dist_info.join("wheel-owned.txt"), b"wheel metadata").unwrap();

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&prefix, &site_packages);
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_file_record(
            conda_path.clone(),
            conda_contents,
        )]);

        uninstall_or_remove_metadata_preserving_conda_paths(
            &dist,
            &layout,
            &prefix,
            Arc::new(registry),
        )
        .await
        .unwrap();

        assert_eq!(
            fs_err::read(prefix.join(conda_path)).unwrap(),
            conda_contents
        );
        assert!(!dist_info.join("wheel-owned.txt").exists());
        assert!(dist_info.is_dir());
    }

    #[tokio::test]
    async fn legacy_egg_uninstall_is_rejected_before_removing_conda_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let package = site_packages.join("example");
        let egg_info = site_packages.join("example-1.0.egg-info");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/example/module.py");
        let conda_contents = b"conda module";
        fs_err::create_dir_all(&package).unwrap();
        fs_err::create_dir(&egg_info).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        fs_err::write(egg_info.join("top_level.txt"), b"example\n").unwrap();

        let dist = installed_egg_info_directory(egg_info.clone());
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_file_record(
            conda_path.clone(),
            conda_contents,
        )]);

        let error = uninstall_preserving_conda_paths(
            &dist,
            &layout(&prefix, &site_packages),
            &prefix,
            Arc::new(registry),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("legacy Python distribution"));
        assert_eq!(
            fs_err::read(prefix.join(conda_path)).unwrap(),
            conda_contents
        );
        assert!(egg_info.is_dir());
    }

    #[test]
    fn metadata_tree_cleanup_requires_relink_for_clobbered_conda_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let dist_info = prefix.join("lib/python3.12/site-packages/example-1.0.dist-info");
        let conda_path =
            PathBuf::from("lib/python3.12/site-packages/example-1.0.dist-info/conda-owned.txt");
        let installed_path = prefix.join(&conda_path);
        let conda_contents = b"conda metadata";
        fs_err::create_dir_all(&dist_info).unwrap();
        fs_err::write(&installed_path, b"PyPI metadata").unwrap();
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_file_record(
            conda_path,
            conda_contents,
        )]);

        let packages = registry
            .packages_requiring_reinstall_for_tree(&prefix, &dist_info)
            .unwrap();
        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));
        let error =
            super::remove_tree_preserving_conda_paths(&dist_info, &prefix, &registry).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("before Conda packages are relinked")
        );
        assert!(error.to_string().contains("conda-pkg"));
        assert_eq!(fs_err::read(installed_path).unwrap(), b"PyPI metadata");
    }

    #[test]
    fn metadata_tree_cleanup_requires_relink_for_missing_conda_file() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let dist_info = prefix.join("lib/python3.12/site-packages/example-1.0.dist-info");
        let wheel_path = dist_info.join("wheel-owned.txt");
        let conda_path =
            PathBuf::from("lib/python3.12/site-packages/example-1.0.dist-info/conda-owned.txt");
        fs_err::create_dir_all(&dist_info).unwrap();
        fs_err::write(&wheel_path, b"wheel metadata").unwrap();
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_file_record(
            conda_path,
            b"conda metadata",
        )]);

        let packages = registry
            .packages_requiring_reinstall_for_tree(&prefix, &dist_info)
            .unwrap();
        assert!(packages.contains(&PackageName::new_unchecked("conda-pkg")));
        assert!(super::remove_tree_preserving_conda_paths(&dist_info, &prefix, &registry).is_err());
        assert_eq!(fs_err::read(wheel_path).unwrap(), b"wheel metadata");
    }

    #[cfg(unix)]
    #[test]
    fn metadata_tree_cleanup_rejects_symlinked_ancestor() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let outside = temp_dir.path().join("outside");
        let outside_dist_info = outside.join("example-1.0.dist-info");
        let wheel_path = outside_dist_info.join("wheel-owned.txt");
        fs_err::create_dir_all(&site_packages).unwrap();
        fs_err::create_dir_all(&outside_dist_info).unwrap();
        fs_err::write(&wheel_path, b"must survive").unwrap();
        symlink(&outside, site_packages.join("alias")).unwrap();
        let aliased_dist_info = site_packages.join("alias/example-1.0.dist-info");

        let error = super::remove_tree_preserving_conda_paths(
            &aliased_dist_info,
            &prefix,
            &PypiCondaClobberRegistry::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("symbolic link or reparse point"));
        assert_eq!(fs_err::read(wheel_path).unwrap(), b"must survive");
    }

    #[cfg(unix)]
    #[test]
    fn metadata_tree_cleanup_rejects_symlink_hidden_by_parent_component() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let outside = temp_dir.path().join("outside");
        let nested = outside.join("nested");
        let victim = outside.join("victim");
        let wheel_path = victim.join("wheel-owned.txt");
        fs_err::create_dir_all(&site_packages).unwrap();
        fs_err::create_dir_all(&nested).unwrap();
        fs_err::create_dir_all(&victim).unwrap();
        fs_err::write(&wheel_path, b"must survive").unwrap();
        symlink(&nested, site_packages.join("alias")).unwrap();
        let hidden_path = site_packages.join("alias/../victim");

        let error = super::remove_tree_preserving_conda_paths(
            &hidden_path,
            &prefix,
            &PypiCondaClobberRegistry::default(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("non-normal path component"));
        assert_eq!(fs_err::read(wheel_path).unwrap(), b"must survive");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uninstall_prunes_paths_through_prefix_symlink_alias() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let real_prefix = temp_dir.path().join("real-prefix");
        let prefix_alias = temp_dir.path().join("prefix-alias");
        let site_packages = real_prefix.join("lib/python3.12/site-packages");
        let package = site_packages.join("pkg");
        let dist_info = site_packages.join("example-1.0.dist-info");
        let conda_directory = PathBuf::from("lib/python3.12/site-packages/pkg");
        fs_err::create_dir_all(&package).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(package.join("wheel.py"), b"wheel module").unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "pkg/wheel.py,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();
        symlink(&real_prefix, &prefix_alias).unwrap();

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&real_prefix, &site_packages);
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_directory_record(
            conda_directory,
        )]);

        uninstall_preserving_conda_paths(&dist, &layout, &prefix_alias, Arc::new(registry))
            .await
            .unwrap();

        assert!(package.is_dir());
        assert!(!package.join("wheel.py").exists());
        assert!(!dist_info.exists());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn uninstall_preserves_unicode_normalized_directory_and_pycache() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let composed_package = site_packages.join("package-\u{e9}");
        let decomposed_package = site_packages.join("package-e\u{301}");
        let dist_info = site_packages.join("example-1.0.dist-info");
        let conda_directory = PathBuf::from("lib/python3.12/site-packages/package-\u{e9}");
        let conda_pyc = conda_directory.join("__pycache__/module-\u{e9}.pyc");
        let conda_contents = b"conda bytecode";
        fs_err::create_dir_all(composed_package.join("__pycache__")).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(prefix.join(&conda_pyc), conda_contents).unwrap();
        if !decomposed_package.is_dir() {
            eprintln!(
                "skipping Unicode normalization test on a normalization-sensitive filesystem"
            );
            return;
        }
        fs_err::write(decomposed_package.join("wheel.py"), b"wheel module").unwrap();
        fs_err::write(
            composed_package.join("__pycache__/stale.pyc"),
            b"stale bytecode",
        )
        .unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "package-e\u{301}/wheel.py,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&prefix, &site_packages);
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[
            conda_directory_record(conda_directory),
            conda_file_record(conda_pyc.clone(), conda_contents),
        ]);

        uninstall_preserving_conda_paths(&dist, &layout, &prefix, Arc::new(registry))
            .await
            .unwrap();

        assert!(composed_package.is_dir());
        assert!(!decomposed_package.join("wheel.py").exists());
        assert_eq!(
            fs_err::read(prefix.join(conda_pyc)).unwrap(),
            conda_contents
        );
        assert!(!composed_package.join("__pycache__/stale.pyc").exists());
        assert!(!dist_info.exists());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn uninstall_preserves_pycache_through_case_insensitive_parent_alias() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("Lib/site-packages");
        let package = site_packages.join("MixedCase");
        let dist_info = site_packages.join("example-1.0.dist-info");
        let conda_path = PathBuf::from("Lib/site-packages/MixedCase/__pycache__/conda.pyc");
        let conda_contents = b"conda bytecode";
        let other_path = package.join("other.py");
        fs_err::create_dir_all(package.join("__pycache__")).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(prefix.join(&conda_path), conda_contents).unwrap();
        fs_err::write(&other_path, b"wheel module").unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "mixedcase/other.py,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();
        if !site_packages.join("mixedcase").is_dir() {
            eprintln!("skipping case-insensitive path test on a case-sensitive filesystem");
            return;
        }

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&prefix, &site_packages);
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_file_record(
            conda_path.clone(),
            conda_contents,
        )]);

        uninstall_preserving_conda_paths(&dist, &layout, &prefix, Arc::new(registry))
            .await
            .unwrap();

        assert!(!other_path.exists());
        assert_eq!(
            fs_err::read(prefix.join(conda_path)).unwrap(),
            conda_contents
        );
        assert!(!dist_info.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uninstall_does_not_follow_record_symlink_ancestor() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let real_package = temp_dir.path().join("outside-package");
        let dist_info = site_packages.join("example-1.0.dist-info");
        fs_err::create_dir_all(&site_packages).unwrap();
        fs_err::create_dir(&real_package).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(real_package.join("module.py"), b"must survive").unwrap();
        symlink(&real_package, site_packages.join("example")).unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "example/module.py,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();

        let dist = installed_dist(dist_info);
        let layout = layout(&prefix, &site_packages);

        uninstall_preserving_conda_paths(
            &dist,
            &layout,
            &prefix,
            Arc::new(PypiCondaClobberRegistry::default()),
        )
        .await
        .unwrap();

        assert!(
            real_package.join("module.py").is_file(),
            "uninstall must not follow a symlinked RECORD ancestor"
        );
    }

    #[cfg(any(unix, windows))]
    #[tokio::test]
    async fn uninstall_does_not_follow_symlink_hidden_by_parent_component() {
        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let outside = temp_dir.path().join("outside");
        let nested = outside.join("nested");
        let victim = outside.join("victim");
        let alias = site_packages.join("alias");
        let dist_info = site_packages.join("example-1.0.dist-info");
        fs_err::create_dir_all(&site_packages).unwrap();
        fs_err::create_dir_all(&nested).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(&victim, b"must survive").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&nested, &alias).unwrap();
        #[cfg(windows)]
        if let Err(err) = std::os::windows::fs::symlink_dir(&nested, &alias) {
            if err.kind() == std::io::ErrorKind::PermissionDenied {
                eprintln!("skipping reparse-point test without symlink permission");
                return;
            }
            panic!("failed to create directory symlink: {err}");
        }

        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "alias/../victim,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();

        let dist = installed_dist(dist_info.clone());
        let layout = layout(&prefix, &site_packages);
        let registry = Arc::new(PypiCondaClobberRegistry::default());
        let records = [RecordEntry {
            path: "alias/../victim".to_string(),
            hash: None,
            size: None,
        }];
        let protection = registry
            .conda_owned_record_paths(&prefix, &site_packages, &records)
            .unwrap();
        assert!(protection.unsafe_to_remove.contains("alias/../victim"));

        uninstall_preserving_conda_paths(&dist, &layout, &prefix, registry)
            .await
            .unwrap();

        assert_eq!(fs_err::read(&victim).unwrap(), b"must survive");
        assert!(!dist_info.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn uninstall_preserves_conda_owned_symlink_leaf() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let package = site_packages.join("example");
        let dist_info = site_packages.join("example-1.0.dist-info");
        let conda_path = PathBuf::from("lib/python3.12/site-packages/example/module.py");
        let target = b"../target.py";
        fs_err::create_dir_all(&package).unwrap();
        fs_err::create_dir(&dist_info).unwrap();
        fs_err::write(site_packages.join("target.py"), b"target").unwrap();
        symlink(
            std::str::from_utf8(target).unwrap(),
            prefix.join(&conda_path),
        )
        .unwrap();
        fs_err::write(
            dist_info.join("RECORD"),
            concat!(
                "example/module.py,,\n",
                "example-1.0.dist-info/METADATA,,\n",
                "example-1.0.dist-info/RECORD,,\n",
            ),
        )
        .unwrap();
        fs_err::write(dist_info.join("METADATA"), b"Metadata-Version: 2.1\n").unwrap();

        let dist = installed_dist(dist_info);
        let layout = layout(&prefix, &site_packages);
        let registry = PypiCondaClobberRegistry::with_conda_packages(&[conda_softlink_record(
            conda_path.clone(),
            target,
        )]);

        uninstall_preserving_conda_paths(&dist, &layout, &prefix, Arc::new(registry))
            .await
            .unwrap();

        let installed_path = prefix.join(conda_path);
        assert!(
            fs_err::symlink_metadata(&installed_path)
                .unwrap()
                .file_type()
                .is_symlink(),
            "uninstall must preserve a verified conda symlink"
        );
        assert_eq!(
            fs_err::read_link(installed_path).unwrap(),
            PathBuf::from(std::str::from_utf8(target).unwrap())
        );
    }

    #[test]
    fn pycache_cleanup_preserves_only_conda_owned_entries() {
        let temp_dir = tempfile::tempdir().unwrap();
        let pycache = temp_dir.path().join("__pycache__");
        fs_err::create_dir(&pycache).unwrap();
        fs_err::write(pycache.join("conda-owned.pyc"), "conda").unwrap();
        fs_err::write(pycache.join("stale-pypi.pyc"), "pypi").unwrap();
        fs_err::create_dir(pycache.join("stale-package")).unwrap();
        fs_err::write(pycache.join("stale-package/conda-owned.pyc"), "conda").unwrap();
        fs_err::write(pycache.join("stale-package/stale.pyc"), "pypi").unwrap();

        let protected = [
            pycache.join("conda-owned.pyc"),
            pycache.join("stale-package/conda-owned.pyc"),
        ]
        .into_iter()
        .collect::<AHashSet<_>>();
        let summary = clean_unowned_pycache_entries(&pycache, &protected).unwrap();

        assert!(pycache.join("conda-owned.pyc").is_file());
        assert!(!pycache.join("stale-pypi.pyc").exists());
        assert!(pycache.join("stale-package/conda-owned.pyc").is_file());
        assert!(!pycache.join("stale-package/stale.pyc").exists());
        assert_eq!(summary.file_count, 2);
        assert_eq!(summary.dir_count, 0);
    }

    #[cfg(unix)]
    #[test]
    fn pycache_cleanup_does_not_follow_directory_symlink() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let outside = temp_dir.path().join("outside");
        fs_err::create_dir(&outside).unwrap();
        fs_err::write(outside.join("keep.pyc"), "outside").unwrap();
        let pycache = temp_dir.path().join("__pycache__");
        symlink(&outside, &pycache).unwrap();

        let summary = clean_unowned_pycache_entries(&pycache, &AHashSet::new()).unwrap();

        assert!(outside.join("keep.pyc").is_file());
        assert!(pycache.is_symlink());
        assert_eq!(summary.file_count, 0);
        assert_eq!(summary.dir_count, 0);
    }
}
