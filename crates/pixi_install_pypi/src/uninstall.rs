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
        let protected_paths = conda_registry.conda_owned_record_paths(
            &prefix,
            site_packages,
            records.iter().map(|record| record.path.as_str()),
        );

        if protected_paths.is_empty() {
            return uv_install_wheel::uninstall_wheel(dist.install_path(), &dist, &layout);
        }

        tracing::debug!(
            "Preserving {} conda-owned path(s) while uninstalling {}",
            protected_paths.len(),
            dist
        );

        let temporary_dist_info = TemporaryDistInfo::new(site_packages)?;
        temporary_dist_info.write_record(
            records
                .into_iter()
                .filter(|record| !protected_paths.contains(&record.path)),
        )?;

        uv_install_wheel::uninstall_wheel(&temporary_dist_info.path, &dist, &layout)
    })
    .await?
    .map_err(UninstallError::from)
}
