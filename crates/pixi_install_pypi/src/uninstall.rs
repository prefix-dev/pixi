use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use uv_distribution_types::{InstalledDist, InstalledDistKind};
use uv_install_wheel::{Layout, RecordEntry, Uninstall};
use uv_installer::UninstallError;

use crate::conda_pypi_clobber::PypiCondaClobberRegistry;

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
            let file_type = entry.file_type()?;
            if file_type.is_dir() && !file_type.is_symlink() {
                let contains_protected_path = protected_entries
                    .iter()
                    .any(|path| path == &relative_path || path.starts_with(&relative_path));
                if contains_protected_path {
                    clean_directory(&entry.path(), &relative_path, protected_entries, summary)?;
                } else {
                    fs_err::remove_dir_all(entry.path())?;
                    summary.dir_count += 1;
                }
            } else if !protected_entries.contains(&relative_path) {
                fs_err::remove_file(entry.path())?;
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

/// Uninstall a distribution without deleting wheel files owned by currently
/// installed conda packages.
pub(crate) async fn uninstall_preserving_conda_paths(
    dist: &InstalledDist,
    layout: &Layout,
    prefix: &Path,
    conda_registry: Arc<PypiCondaClobberRegistry>,
) -> Result<Uninstall, UninstallError> {
    if !matches!(
        &dist.kind,
        InstalledDistKind::Registry(_) | InstalledDistKind::Url(_)
    ) {
        return uv_installer::uninstall(dist, layout).await;
    }

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

        if protection.owned.is_empty() && protection.cleanup_sensitive.is_empty() {
            return uv_install_wheel::uninstall_wheel(dist.install_path(), &dist, &layout);
        }

        tracing::debug!(
            "Preserving {} conda-owned path(s) while uninstalling {}",
            protection.owned.len(),
            dist
        );

        let mut filtered_records = Vec::with_capacity(records.len());
        let mut cleanup_sensitive_paths = Vec::new();
        for record in records {
            if protection.owned.contains(&record.path) {
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
        for record_path in cleanup_sensitive_paths {
            let path = site_packages.join(record_path);
            match fs_err::remove_file(&path) {
                Ok(()) => summary.file_count += 1,
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

        Ok(summary)
    })
    .await?
    .map_err(UninstallError::from)
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr, sync::Arc};

    use ahash::AHashSet;
    use uv_distribution_types::{InstalledDist, InstalledDistKind, InstalledRegistryDist};
    use uv_install_wheel::Layout;

    use super::{clean_unowned_pycache_entries, uninstall_preserving_conda_paths};
    use crate::conda_pypi_clobber::PypiCondaClobberRegistry;

    #[cfg(unix)]
    #[tokio::test]
    async fn uninstall_does_not_follow_record_symlink_ancestor() {
        use std::os::unix::fs::symlink;

        let temp_dir = tempfile::tempdir().unwrap();
        let prefix = temp_dir.path().join("prefix");
        let site_packages = prefix.join("lib/python3.12/site-packages");
        let real_package = prefix.join("real-package");
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

        let dist = InstalledDist::from(InstalledDistKind::Registry(InstalledRegistryDist {
            name: uv_normalize::PackageName::from_str("example").unwrap(),
            version: uv_pep440::Version::from_str("1.0").unwrap(),
            path: dist_info.clone().into(),
            cache_info: None,
            build_info: None,
        }));
        let layout = Layout {
            sys_executable: prefix.join("bin/python"),
            python_version: (3, 12),
            os_name: std::env::consts::OS.to_string(),
            scheme: uv_pypi_types::Scheme {
                purelib: site_packages.clone(),
                platlib: site_packages.clone(),
                scripts: prefix.join("bin"),
                data: prefix.clone(),
                include: prefix.join("include"),
            },
        };

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
            PathBuf::from("conda-owned.pyc"),
            PathBuf::from("stale-package/conda-owned.pyc"),
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
