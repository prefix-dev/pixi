//! Resolution selection for PEP 723 script environments.
//!
//! A script's adjacent lock file is optional exact resolution authority. When
//! it is absent, Pixi keeps a disposable cached resolution under the script's
//! environment cache directory. Missing, corrupt, or unsupported cache state
//! is treated as a cache miss.

use std::{path::PathBuf, sync::Arc};

use miette::{Context, IntoDiagnostic};
use pixi_manifest::HasWorkspaceManifest;
use rattler_lock::LockFile;
use serde::{Deserialize, Serialize};

use super::Workspace;
use crate::{
    environment::LockFileUsage,
    lock_file::{LockFileDerivedData, UpdateLockFileOptions},
};

const CACHE_VERSION: u32 = 1;
const CACHE_FILE: &str = "script-resolution-v1.json";

#[derive(Serialize, Deserialize)]
struct StoredResolution {
    version: u32,
    lock_file: String,
}

/// Options for resolving a script environment.
pub struct ScriptEnvironmentOptions {
    pub progress: Option<Arc<pixi_reporters::TopLevelProgress>>,
    pub lock_file_usage: LockFileUsage,
    pub no_install: bool,
}

impl Workspace {
    /// Resolve a script from its adjacent lock file or disposable cache.
    ///
    /// This method never writes the adjacent lock file. A resolved change is
    /// caller-owned until the script environment Module commits it.
    pub async fn resolve_script_environment(
        &self,
        options: ScriptEnvironmentOptions,
    ) -> miette::Result<LockFileDerivedData<'_>> {
        if !self.is_script() {
            return Err(miette::miette!(
                "script environment resolution requires a PEP 723 workspace"
            ));
        }

        let ScriptEnvironmentOptions {
            progress,
            lock_file_usage,
            no_install,
        } = options;
        let cache_path = self.pixi_dir().join(CACHE_FILE);
        let adjacent_lock_exists = self
            .persistent_lock_file_path()
            .is_some_and(|path| path.is_file());

        if !adjacent_lock_exists {
            match lock_file_usage {
                LockFileUsage::Locked => {
                    return Err(miette::miette!(
                        help = "Create one with `pixi lock --script <PATH>`.",
                        "no lock file exists for the script, but `--locked` was requested"
                    ));
                }
                LockFileUsage::Frozen => {
                    return Err(miette::miette!(
                        help = "Create one with `pixi lock --script <PATH>`.",
                        "no lock file exists for the script, but `--frozen` was requested"
                    ));
                }
                LockFileUsage::Update | LockFileUsage::DryRun => {}
            }
        }

        let baseline = if adjacent_lock_exists {
            let loaded = self.load_lock_file().await?;
            if matches!(
                lock_file_usage,
                LockFileUsage::Locked | LockFileUsage::Frozen
            ) {
                loaded.into_lock_file()?
            } else {
                loaded.into_lock_file_or_empty_with_warning()
            }
        } else {
            load_cached_resolution(cache_path.clone(), self)
                .await
                .unwrap_or_default()
        };

        let (resolved, _) = self
            .update_lock_file_from_lock_file(
                progress,
                UpdateLockFileOptions {
                    lock_file_usage,
                    no_install,
                    max_concurrent_solves: self.config().max_concurrent_solves(),
                    ..Default::default()
                },
                baseline,
            )
            .await?;

        if !adjacent_lock_exists
            && lock_file_usage != LockFileUsage::DryRun
            && let Err(error) = store_cached_resolution(cache_path, resolved.as_lock_file()).await
        {
            tracing::warn!(
                %error,
                "failed to cache the script resolution; the environment remains usable"
            );
        }

        Ok(resolved)
    }
}

async fn store_cached_resolution(path: PathBuf, lock_file: &LockFile) -> miette::Result<()> {
    let lock_file = lock_file
        .render_to_string()
        .into_diagnostic()
        .context("failed to serialize the cached script resolution")?;
    let contents = serde_json::to_vec(&StoredResolution {
        version: CACHE_VERSION,
        lock_file,
    })
    .into_diagnostic()
    .context("failed to serialize cached script resolution state")?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to create the script environment cache `{}`",
                    parent.display()
                )
            })?;
    }
    pixi_utils::atomic_write::atomic_write(&path, contents)
        .await
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "failed to write script resolution cache `{}`",
                path.display()
            )
        })
}

async fn load_cached_resolution(path: PathBuf, workspace: &Workspace) -> Option<LockFile> {
    let contents = match tokio::fs::read_to_string(&path).await {
        Ok(contents) => contents,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "script resolution cache is unavailable");
            return None;
        }
    };
    let stored: StoredResolution = match serde_json::from_str(&contents) {
        Ok(stored) => stored,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "script resolution cache is invalid");
            return None;
        }
    };
    if stored.version != CACHE_VERSION {
        tracing::debug!(
            path = %path.display(),
            version = stored.version,
            "script resolution cache has an unsupported version"
        );
        return None;
    }
    let lock_file = match LockFile::from_str_with_base_directory(
        &stored.lock_file,
        Some(workspace.root()),
    ) {
        Ok(lock_file) => lock_file,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "script resolution cache contains an invalid lock file");
            return None;
        }
    };
    Some(crate::lock_file::align_platform_names(
        lock_file,
        workspace.workspace_manifest(),
        workspace.root(),
    ))
}

#[cfg(test)]
mod tests {
    use pixi_config::{CacheConfig, Config};
    use pixi_manifest::script::ScriptManifest;
    use rattler_conda_types::NamedChannelOrUrl;

    use super::*;

    fn script_workspace(root: &std::path::Path, cache: &std::path::Path) -> Workspace {
        let path = root.join("example.py");
        fs_err::write(
            &path,
            "# /// script\n# dependencies = []\n# ///\nprint(\"hello\")\n",
        )
        .unwrap();
        let script = ScriptManifest::from_path(path).unwrap().unwrap();
        Workspace::from_script(
            script,
            Config {
                default_channels: vec![NamedChannelOrUrl::Name("testing".into())],
                cache: CacheConfig {
                    exec_environments: Some(cache.to_owned()),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap()
        .value
    }

    #[tokio::test]
    async fn cached_resolution_round_trips_and_invalid_state_is_a_miss() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let path = workspace.pixi_dir().join(CACHE_FILE);
        let lock_file = LockFile::default();

        assert!(
            load_cached_resolution(path.clone(), &workspace)
                .await
                .is_none()
        );
        store_cached_resolution(path.clone(), &lock_file)
            .await
            .unwrap();
        let loaded = load_cached_resolution(path.clone(), &workspace)
            .await
            .unwrap();
        assert_eq!(
            loaded.render_to_string().unwrap(),
            lock_file.render_to_string().unwrap()
        );

        for invalid in [
            "not json".to_owned(),
            serde_json::json!({
                "version": CACHE_VERSION + 1,
                "lock_file": lock_file.render_to_string().unwrap(),
            })
            .to_string(),
            serde_json::json!({
                "version": CACHE_VERSION,
                "lock_file": "not a lock file",
            })
            .to_string(),
        ] {
            fs_err::write(&path, invalid).unwrap();
            assert!(
                load_cached_resolution(path.clone(), &workspace)
                    .await
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn cache_is_not_lock_file_authority() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let path = workspace.pixi_dir().join(CACHE_FILE);
        store_cached_resolution(path, &LockFile::default())
            .await
            .unwrap();

        for lock_file_usage in [LockFileUsage::Locked, LockFileUsage::Frozen] {
            let error = workspace
                .resolve_script_environment(ScriptEnvironmentOptions {
                    progress: None,
                    lock_file_usage,
                    no_install: true,
                })
                .await
                .err()
                .expect("an adjacent lock file is required");
            assert!(error.to_string().contains("no lock file exists"));
        }
    }
}
