//! Resolution selection for PEP 723 script environments.
//!
//! A script's adjacent lock file is optional exact resolution authority. When
//! it is absent, Pixi keeps a disposable cached resolution under the script's
//! environment cache directory. Missing, corrupt, or unsupported cache state
//! is treated as a cache miss.

use std::{path::PathBuf, sync::Arc};

use async_fd_lock::LockWrite;
use miette::{Context, IntoDiagnostic};
use pixi_manifest::HasWorkspaceManifest;
use rattler_lock::LockFile;
use serde::{Deserialize, Serialize};

use super::Workspace;
use crate::{
    environment::{InstallFilter, LockFileUsage},
    lock_file::{
        LockFileDerivedData, ReinstallPackages, UpdateLockFileOptions, UpdateMode,
        shorten_platform_names,
    },
};

const CACHE_VERSION: u32 = 1;
const CACHE_FILE: &str = "script-resolution-v1.json";

#[derive(Serialize, Deserialize)]
struct StoredResolution {
    version: u32,
    lock_file: String,
}

struct MutationGuard {
    _guard: async_fd_lock::RwLockWriteGuard<tokio::fs::File>,
}

impl MutationGuard {
    async fn acquire(path: PathBuf) -> miette::Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .into_diagnostic()
                .wrap_err_with(|| {
                    format!(
                        "failed to create the script environment directory `{}`",
                        parent.display()
                    )
                })?;
        }
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .await
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to open the script environment lock `{}`",
                    path.display()
                )
            })?;
        let guard = file
            .lock_write()
            .await
            .map_err(std::io::Error::from)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!("failed to lock the script environment `{}`", path.display())
            })?;
        Ok(Self { _guard: guard })
    }
}

pub(super) struct ScriptChange {
    script_path: PathBuf,
    lock_file_path: PathBuf,
    cache_path: PathBuf,
    mutation_lock_path: PathBuf,
    source_before: Vec<u8>,
    lock_file_before: Option<Vec<u8>>,
}

pub(super) struct CommitCandidate<'w> {
    change: ScriptChange,
    source: String,
    resolution: LockFileDerivedData<'w>,
}

pub(super) struct CommittedChange<'w> {
    source: String,
    resolution: LockFileDerivedData<'w>,
}

pub(super) struct ReadyEnvironment<'w> {
    source: String,
    resolution: LockFileDerivedData<'w>,
}

impl ScriptChange {
    pub(super) fn candidate<'w>(
        self,
        source: String,
        resolution: LockFileDerivedData<'w>,
    ) -> CommitCandidate<'w> {
        CommitCandidate {
            change: self,
            source,
            resolution,
        }
    }

    pub(super) async fn commit_metadata(self, source: String) -> miette::Result<String> {
        let _mutation_guard = MutationGuard::acquire(self.mutation_lock_path.clone()).await?;
        self.ensure_current()?;

        match fs_err::remove_file(&self.cache_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).into_diagnostic().wrap_err_with(|| {
                    format!(
                        "failed to invalidate the script resolution cache `{}`",
                        self.cache_path.display()
                    )
                });
            }
        }
        pixi_utils::atomic_write::atomic_write_sync_strict(&self.script_path, &source)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to commit script metadata `{}`",
                    self.script_path.display()
                )
            })?;
        Ok(source)
    }

    fn ensure_current(&self) -> miette::Result<()> {
        let source = fs_err::read(&self.script_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to read script metadata `{}`",
                    self.script_path.display()
                )
            })?;
        let lock_file = read_optional_sync(&self.lock_file_path)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to read script lock file `{}`",
                    self.lock_file_path.display()
                )
            })?;
        if source != self.source_before || lock_file != self.lock_file_before {
            return Err(miette::miette!(
                help = "Retry the command against the updated script.",
                "the script environment changed while dependencies were being resolved"
            ));
        }
        Ok(())
    }
}

impl<'w> CommitCandidate<'w> {
    pub(super) async fn commit(self) -> miette::Result<CommittedChange<'w>> {
        let _mutation_guard =
            MutationGuard::acquire(self.change.mutation_lock_path.clone()).await?;
        self.change.ensure_current()?;

        let lock_file = shorten_platform_names(
            self.resolution.lock_file.clone(),
            self.resolution.workspace.workspace_manifest(),
            self.resolution.workspace.root(),
        )
        .render_to_string()
        .into_diagnostic()
        .wrap_err("failed to serialize the script lock file")?;
        pixi_utils::atomic_write::atomic_write_sync_strict(&self.change.script_path, &self.source)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to commit script metadata `{}`",
                    self.change.script_path.display()
                )
            })?;
        pixi_utils::atomic_write::atomic_write_sync_strict(&self.change.lock_file_path, lock_file)
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to commit script lock file `{}`",
                    self.change.lock_file_path.display()
                )
            })?;

        Ok(CommittedChange {
            source: self.source,
            resolution: self.resolution,
        })
    }
}

impl<'w> CommittedChange<'w> {
    pub(super) fn source(&self) -> &str {
        &self.source
    }

    pub(super) async fn ensure_prefix(self, install: bool) -> miette::Result<ReadyEnvironment<'w>> {
        if install {
            let environment = self.resolution.workspace.default_environment();
            if environment.best_declared_platform().is_some() {
                self.resolution
                    .prefix(
                        &environment,
                        UpdateMode::Revalidate,
                        &ReinstallPackages::default(),
                        &InstallFilter::default(),
                    )
                    .await?;
            } else {
                tracing::info!(
                    "Skipping prefix installation: no platform supported by environment '{}' matches the current system",
                    environment.name()
                );
            }
        }

        Ok(ReadyEnvironment {
            source: self.source,
            resolution: self.resolution,
        })
    }
}

impl<'w> ReadyEnvironment<'w> {
    pub(super) fn into_parts(self) -> (String, LockFileDerivedData<'w>) {
        (self.source, self.resolution)
    }
}

/// Options for resolving a script environment.
pub struct ScriptEnvironmentOptions {
    pub progress: Option<Arc<pixi_reporters::TopLevelProgress>>,
    pub lock_file_usage: LockFileUsage,
    pub no_install: bool,
}

impl Workspace {
    pub(super) async fn begin_script_change(&self) -> miette::Result<(ScriptChange, LockFile)> {
        let script_path = self.workspace.provenance.path.clone();
        let lock_file_path = self
            .persistent_lock_file_path()
            .ok_or_else(|| miette::miette!("transient scripts cannot commit dependency changes"))?;
        let cache_path = self.pixi_dir().join(CACHE_FILE);
        let mutation_lock_path = self.pixi_dir().join(".script-environment.lock");
        let _mutation_guard = MutationGuard::acquire(mutation_lock_path.clone()).await?;
        let source_before = tokio::fs::read(&script_path)
            .await
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to read script `{}`", script_path.display()))?;
        let lock_file_before = read_optional(&lock_file_path)
            .await
            .into_diagnostic()
            .wrap_err_with(|| {
                format!(
                    "failed to read script lock file `{}`",
                    lock_file_path.display()
                )
            })?;
        let lock_file = self
            .load_lock_file()
            .await?
            .into_lock_file_or_empty_with_warning();

        Ok((
            ScriptChange {
                script_path,
                lock_file_path,
                cache_path,
                mutation_lock_path,
                source_before,
                lock_file_before,
            },
            lock_file,
        ))
    }

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

async fn read_optional(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    match tokio::fs::read(path).await {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_optional_sync(path: &std::path::Path) -> std::io::Result<Option<Vec<u8>>> {
    match fs_err::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
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

    #[tokio::test]
    async fn metadata_commit_invalidates_cached_resolution() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let cache_path = workspace.pixi_dir().join(CACHE_FILE);
        store_cached_resolution(cache_path.clone(), &LockFile::default())
            .await
            .unwrap();
        let (change, _) = workspace.begin_script_change().await.unwrap();
        let source = "# /// script\n# dependencies = [\"rich\"]\n# ///\nprint(\"hello\")\n";

        change.commit_metadata(source.to_owned()).await.unwrap();

        assert_eq!(
            fs_err::read_to_string(&workspace.workspace.provenance.path).unwrap(),
            source
        );
        assert!(!cache_path.exists());
    }

    #[tokio::test]
    async fn metadata_commit_detects_a_concurrent_change() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let (change, _) = workspace.begin_script_change().await.unwrap();
        let concurrent = "# /// script\n# dependencies = [\"click\"]\n# ///\nprint(\"changed\")\n";
        fs_err::write(&workspace.workspace.provenance.path, concurrent).unwrap();

        let error = change
            .commit_metadata(
                "# /// script\n# dependencies = [\"rich\"]\n# ///\nprint(\"hello\")\n".to_owned(),
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("script environment changed"));
        assert_eq!(
            fs_err::read_to_string(&workspace.workspace.provenance.path).unwrap(),
            concurrent
        );
    }

    #[tokio::test]
    async fn resolution_is_committed_before_it_can_become_ready() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let (change, _) = workspace.begin_script_change().await.unwrap();
        let command_dispatcher = workspace.command_dispatcher_builder().unwrap().finish();
        let resolution = LockFileDerivedData::from_input_lock_file(
            &workspace,
            LockFile::default(),
            command_dispatcher.package_cache().clone(),
            command_dispatcher,
            pixi_glob::GlobHashCache::default(),
        );
        let source = fs_err::read_to_string(&workspace.workspace.provenance.path).unwrap();

        let committed = change.candidate(source, resolution).commit().await.unwrap();
        assert!(workspace.lock_file_path().is_file());
        let ready = committed.ensure_prefix(false).await.unwrap();
        let (_, resolution) = ready.into_parts();
        assert_eq!(
            resolution.as_lock_file().render_to_string().unwrap(),
            LockFile::default().render_to_string().unwrap()
        );
    }
}
