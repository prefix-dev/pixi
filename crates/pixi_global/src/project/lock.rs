use async_fd_lock::{LockWrite, RwLockWriteGuard};
use miette::IntoDiagnostic;
use tokio::fs::{File, OpenOptions};

use super::Project;

pub struct GlobalManifestLock {
    _guard: RwLockWriteGuard<File>,
}

impl GlobalManifestLock {
    /// Acquire a cross-process lock on the global manifest to prevent concurrent writes.
    pub async fn lock() -> miette::Result<Self> {
        let manifest_dir = Project::manifest_dir()?;
        tokio::fs::create_dir_all(&manifest_dir)
            .await
            .into_diagnostic()?;
        let lock_path = manifest_dir.join(".pixi-global.toml.lock");

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&lock_path)
            .await
            .into_diagnostic()?;

        let guard = file.lock_write().await.map_err(|_| miette::miette!("Failed to acquire global manifest lock"))?;
        Ok(Self { _guard: guard })
    }
}
