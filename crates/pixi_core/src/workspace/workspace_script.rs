use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use miette::{Context, IntoDiagnostic};
use pixi_manifest::script::ScriptManifest;
use pixi_manifest::script::conda::CondaScriptManifest;
use rattler_lock::LockFile;
use serde::{Deserialize, Serialize};
use xxhash_rust::xxh3::xxh3_64;

use super::{Workspace, WorkspaceStorage};
use crate::{
    environment::LockFileUsage,
    lock_file::{LockFileDerivedData, UpdateLockFileOptions},
};

const RESOLUTION_CACHE_VERSION: u32 = 1;
const RESOLUTION_CACHE_FILE: &str = "script-resolution-v1.json";

/// The on-disk form of a cached script resolution.
#[derive(Serialize, Deserialize)]
struct CachedResolution {
    version: u32,
    lock_file: String,
}

/// The parsed manifest of a script workspace: a PEP 723 Python script or a
/// `conda-script` code file.
#[derive(Debug, Clone)]
pub(super) enum ScriptSource {
    Pep723(Box<ScriptManifest>),
    CondaScript(Box<CondaScriptManifest>),
}

/// Describes where a script reads and writes workspace state.
///
/// Every script has a parsed manifest and a cache directory. A local script
/// also has an adjacent lock-file path. A transient script (e.g., one read from
/// a URL or standard input) does not have one. A script without an adjacent
/// lock file may keep its last resolution in the cache directory.
#[derive(Debug, Clone)]
pub(super) struct WorkspaceScript {
    source: ScriptSource,
    pixi_dir: PathBuf,

    /// The adjacent lock-file path for a local script.
    lock_file_path: Option<PathBuf>,
}

impl WorkspaceScript {
    pub(super) fn for_local(manifest: ScriptManifest, cache_root: &Path) -> Self {
        let pixi_dir = cache_root.join(local_cache_name(manifest.path()));
        let lock_file_path = Some(local_lock_file_path(manifest.path()));
        Self {
            source: ScriptSource::Pep723(Box::new(manifest)),
            pixi_dir,
            lock_file_path,
        }
    }

    pub(super) fn for_local_conda_script(manifest: CondaScriptManifest, cache_root: &Path) -> Self {
        let pixi_dir = cache_root.join(local_cache_name(manifest.path()));
        let lock_file_path = Some(local_lock_file_path(manifest.path()));
        Self {
            source: ScriptSource::CondaScript(Box::new(manifest)),
            pixi_dir,
            lock_file_path,
        }
    }

    pub(super) fn for_transient(
        manifest: ScriptManifest,
        cache_root: &Path,
        cache_name: &str,
        cache_key: &[u8],
        root: &Path,
    ) -> Self {
        Self {
            source: ScriptSource::Pep723(Box::new(manifest)),
            pixi_dir: cache_root.join(transient_cache_name(cache_name, cache_key, root)),
            lock_file_path: None,
        }
    }

    pub(super) fn source(&self) -> &ScriptSource {
        &self.source
    }

    pub(super) fn replace_manifest(&mut self, new_manifest: ScriptManifest) {
        if self.lock_file_path.is_some() {
            self.lock_file_path = Some(local_lock_file_path(new_manifest.path()));
        }
        match &mut self.source {
            ScriptSource::Pep723(manifest) => **manifest = new_manifest,
            ScriptSource::CondaScript(_) => {
                unreachable!("conda-script workspaces reject manifest edits when they are opened")
            }
        }
    }

    pub(super) fn pixi_dir(&self) -> &Path {
        &self.pixi_dir
    }

    pub(super) fn lock_file_path(&self) -> Option<PathBuf> {
        self.lock_file_path.clone()
    }

    fn resolution_cache_path(&self) -> PathBuf {
        self.pixi_dir().join(RESOLUTION_CACHE_FILE)
    }

    /// Loads the cached resolution, or returns `None` when it cannot be used.
    async fn load_cached_resolution(&self, root: &Path) -> Option<LockFile> {
        let path = self.resolution_cache_path();
        let contents = match tokio::fs::read(&path).await {
            Ok(contents) => contents,
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "script resolution cache is unavailable");
                return None;
            }
        };
        let cached: CachedResolution = match serde_json::from_slice(&contents) {
            Ok(cached) => cached,
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "script resolution cache is invalid");
                return None;
            }
        };
        if cached.version != RESOLUTION_CACHE_VERSION {
            tracing::debug!(
                path = %path.display(),
                version = cached.version,
                "script resolution cache has an unsupported version"
            );
            return None;
        }
        match LockFile::from_str_with_base_directory(&cached.lock_file, Some(root)) {
            Ok(lock_file) => Some(lock_file),
            Err(error) => {
                tracing::debug!(path = %path.display(), %error, "script resolution cache contains an invalid lock file");
                None
            }
        }
    }

    /// Writes a lock file to the script's resolution cache.
    async fn write_cached_resolution(&self, lock_file: &LockFile) -> miette::Result<()> {
        let path = self.resolution_cache_path();
        let lock_file = lock_file
            .render_to_string()
            .into_diagnostic()
            .context("failed to serialize the cached script resolution")?;
        let contents = serde_json::to_vec(&CachedResolution {
            version: RESOLUTION_CACHE_VERSION,
            lock_file,
        })
        .into_diagnostic()
        .context("failed to serialize the script resolution cache")?;
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
                    "failed to write the script resolution cache `{}`",
                    path.display()
                )
            })
    }
}

impl Workspace {
    /// Loads the lock-file input for an explicit update.
    ///
    /// Projects and scripts with an adjacent lock file use that lock file. A
    /// script without one uses its cached resolution.
    pub async fn load_lock_file_for_update(&self) -> miette::Result<LockFile> {
        if let WorkspaceStorage::Script(script) = &self.storage
            && !script.lock_file_path().is_some_and(|path| path.is_file())
        {
            return Ok(script
                .load_cached_resolution(self.root())
                .await
                .unwrap_or_default());
        }

        Ok(self
            .load_lock_file()
            .await?
            .into_lock_file_or_empty_with_warning())
    }

    /// Resolves the lock file used by an operational command.
    ///
    /// Projects use their persistent lock file. Scripts use an adjacent lock
    /// file when one exists and a cached resolution otherwise.
    pub async fn resolve_lock_file(
        &self,
        progress: Option<Arc<pixi_reporters::TopLevelProgress>>,
        options: UpdateLockFileOptions,
    ) -> miette::Result<(LockFileDerivedData<'_>, bool)> {
        let WorkspaceStorage::Script(script) = &self.storage else {
            return self.update_lock_file(progress, options).await;
        };

        if script.lock_file_path().is_some_and(|path| path.is_file()) {
            return self.update_lock_file(progress, options).await;
        }

        let lock_file_usage = options.lock_file_usage;
        if matches!(
            lock_file_usage,
            LockFileUsage::Locked | LockFileUsage::Frozen
        ) {
            let option = match lock_file_usage {
                LockFileUsage::Locked => "--locked",
                LockFileUsage::Frozen => "--frozen",
                LockFileUsage::Update | LockFileUsage::DryRun => unreachable!(),
            };
            if script.lock_file_path().is_none() {
                return Err(miette::miette!(
                    "transient scripts cannot use `{option}` because they do not have an adjacent lock file"
                ));
            }
            return Err(miette::miette!(
                help = "Create one with `pixi lock --script <PATH>`.",
                "no lock file exists for the script, but `{option}` was requested"
            ));
        }

        let cached = script.load_cached_resolution(self.root()).await;
        let cache_was_missing = cached.is_none();
        let lock_file = cached.unwrap_or_default();
        let (resolved, updated) = self
            .update_lock_file_from_lock_file(progress, options, lock_file)
            .await?;

        if lock_file_usage != LockFileUsage::DryRun
            && (cache_was_missing || updated)
            && let Err(error) = script
                .write_cached_resolution(resolved.as_lock_file())
                .await
        {
            tracing::warn!(
                %error,
                "failed to cache the script resolution; the environment remains usable"
            );
        }

        Ok((resolved, updated))
    }
}

impl LockFileDerivedData<'_> {
    /// Writes a resolution produced by an explicit update.
    ///
    /// Projects and scripts with an adjacent lock file write that lock file. A
    /// script without one writes its cached resolution.
    pub async fn write_updated_resolution(&self) -> miette::Result<()> {
        if let WorkspaceStorage::Script(script) = &self.workspace.storage
            && !script.lock_file_path().is_some_and(|path| path.is_file())
        {
            return script.write_cached_resolution(&self.lock_file).await;
        }

        self.write_to_disk()
    }
}

fn local_lock_file_path(script_path: &Path) -> PathBuf {
    let mut file_name = script_path
        .file_name()
        .expect("an absolute script path always has a file name")
        .to_os_string();
    file_name.push(".pixi.lock");
    script_path.with_file_name(file_name)
}

fn local_cache_name(path: &Path) -> String {
    let digest = format!("{:016x}", xxh3_64(path.to_string_lossy().as_bytes()));
    let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
        return digest;
    };

    let mut name = String::with_capacity(stem.len().min(100));
    let mut last_was_dash = false;
    for byte in stem.bytes().take(100) {
        if byte.is_ascii_alphanumeric() {
            name.push(byte.to_ascii_lowercase() as char);
            last_was_dash = false;
        } else if !last_was_dash {
            name.push('-');
            last_was_dash = true;
        }
    }
    let name = name.trim_matches('-');
    if name.is_empty() {
        digest
    } else {
        format!("{name}-{digest}")
    }
}

fn transient_cache_name(name: &str, key: &[u8], root: &Path) -> String {
    let mut scoped_key = Vec::with_capacity(root.as_os_str().len() + key.len() + 1);
    scoped_key.extend_from_slice(root.as_os_str().as_encoded_bytes());
    scoped_key.push(0);
    scoped_key.extend_from_slice(key);
    let digest = format!("{:016x}", xxh3_64(&scoped_key));
    let name = name
        .bytes()
        .take(100)
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                byte.to_ascii_lowercase() as char
            } else {
                '-'
            }
        })
        .collect::<String>();
    let name = name.trim_matches('-');
    if name.is_empty() {
        digest
    } else {
        format!("{name}-{digest}")
    }
}

#[cfg(test)]
mod tests {
    use pixi_config::{CacheConfig, Config};
    use rattler_conda_types::NamedChannelOrUrl;

    use super::*;

    const SOURCE: &str =
        "# /// script\n# dependencies = []\n#\n# [tool.pixi.workspace]\n# platforms = []\n# ///\n";

    fn manifest(path: &Path) -> ScriptManifest {
        ScriptManifest::from_source(path, SOURCE.as_bytes())
            .unwrap()
            .unwrap()
    }

    fn script_workspace(root: &Path, cache: &Path) -> Workspace {
        let path = root.join("example.py");
        fs_err::write(&path, SOURCE).unwrap();
        Workspace::from_script(
            ScriptManifest::from_path(path).unwrap().unwrap(),
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

    #[test]
    fn local_script() {
        let directory = tempfile::tempdir().unwrap();
        let cache_root = directory.path().join("cache");
        let script_path = directory.path().join("scripts/Example Script.py");
        let first_manifest = manifest(&script_path);
        let manifest_path = first_manifest.path().to_owned();
        let digest = xxh3_64(manifest_path.to_string_lossy().as_bytes());

        let first = WorkspaceScript::for_local(first_manifest, &cache_root);
        let second = WorkspaceScript::for_local(manifest(&script_path), &cache_root);
        let other_path = directory.path().join("other/Example Script.py");
        let other = WorkspaceScript::for_local(manifest(&other_path), &cache_root);

        assert_eq!(
            Some(manifest_path.with_file_name("Example Script.py.pixi.lock")),
            first.lock_file_path()
        );
        assert_eq!(
            cache_root.join(format!("example-script-{digest:016x}")),
            first.pixi_dir(),
        );
        assert_eq!(first.pixi_dir(), second.pixi_dir());
        assert_ne!(first.pixi_dir(), other.pixi_dir());
    }

    #[test]
    fn transient_script() {
        let directory = tempfile::tempdir().unwrap();
        let cache_root = directory.path().join("cache");
        let script_path = directory.path().join("example.py");
        let root = directory.path().join("first");
        let key = b"first key";
        let mut scoped_key = root.as_os_str().as_encoded_bytes().to_vec();
        scoped_key.push(0);
        scoped_key.extend_from_slice(key);
        let digest = xxh3_64(&scoped_key);
        let first = WorkspaceScript::for_transient(
            manifest(&script_path),
            &cache_root,
            "HTTPS://example.com/example.py",
            key,
            &root,
        );
        let first_again = WorkspaceScript::for_transient(
            manifest(&script_path),
            &cache_root,
            "HTTPS://example.com/example.py",
            key,
            &root,
        );
        let second = WorkspaceScript::for_transient(
            manifest(&script_path),
            &cache_root,
            "HTTPS://example.com/example.py",
            b"second key",
            &root,
        );
        let other_root = WorkspaceScript::for_transient(
            manifest(&script_path),
            &cache_root,
            "HTTPS://example.com/example.py",
            key,
            &directory.path().join("second"),
        );

        assert!(first.lock_file_path().is_none());
        assert_eq!(
            cache_root.join(format!("https---example-com-example-py-{digest:016x}")),
            first.pixi_dir(),
        );
        assert_eq!(first.pixi_dir(), first_again.pixi_dir());
        assert_ne!(first.pixi_dir(), second.pixi_dir());
        assert_ne!(first.pixi_dir(), other_root.pixi_dir());
    }

    #[test]
    fn empty_transient_cache_name_uses_the_digest() {
        let directory = tempfile::tempdir().unwrap();
        let cache_root = directory.path().join("cache");
        let script_path = directory.path().join("example.py");
        let key = b"cache key";
        let root = directory.path().join("root");
        let mut scoped_key = root.as_os_str().as_encoded_bytes().to_vec();
        scoped_key.push(0);
        scoped_key.extend_from_slice(key);
        let script =
            WorkspaceScript::for_transient(manifest(&script_path), &cache_root, "://", key, &root);
        let expected = cache_root.join(format!("{:016x}", xxh3_64(&scoped_key)));

        assert_eq!(expected, script.pixi_dir());
    }

    #[tokio::test]
    async fn cached_resolution_round_trips() {
        let directory = tempfile::tempdir().unwrap();
        let cache_root = directory.path().join("cache");
        let script_path = directory.path().join("example.py");
        let script = WorkspaceScript::for_local(manifest(&script_path), &cache_root);
        let lock_file = LockFile::default();

        assert!(
            script
                .load_cached_resolution(directory.path())
                .await
                .is_none()
        );
        script.write_cached_resolution(&lock_file).await.unwrap();
        let loaded = script
            .load_cached_resolution(directory.path())
            .await
            .unwrap();

        assert_eq!(
            lock_file.render_to_string().unwrap(),
            loaded.render_to_string().unwrap()
        );
    }

    #[tokio::test]
    async fn invalid_cached_resolution_is_a_miss() {
        let directory = tempfile::tempdir().unwrap();
        let cache_root = directory.path().join("cache");
        let script_path = directory.path().join("example.py");
        let script = WorkspaceScript::for_local(manifest(&script_path), &cache_root);
        let path = script.resolution_cache_path();
        fs_err::create_dir_all(path.parent().unwrap()).unwrap();
        let lock_file = LockFile::default().render_to_string().unwrap();

        for invalid in [
            "not json".to_owned(),
            serde_json::json!({
                "version": RESOLUTION_CACHE_VERSION + 1,
                "lock_file": lock_file,
            })
            .to_string(),
            serde_json::json!({
                "version": RESOLUTION_CACHE_VERSION,
                "lock_file": "not a lock file",
            })
            .to_string(),
        ] {
            fs_err::write(&path, invalid).unwrap();
            assert!(
                script
                    .load_cached_resolution(directory.path())
                    .await
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn operational_resolution_writes_the_cache() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let cache_path = workspace.pixi_dir().join(RESOLUTION_CACHE_FILE);

        workspace
            .resolve_lock_file(
                None,
                UpdateLockFileOptions {
                    lock_file_usage: LockFileUsage::Update,
                    no_install: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(cache_path.is_file());
        assert!(!workspace.lock_file_path().exists());
    }

    #[tokio::test]
    async fn cached_resolution_is_not_lock_file_authority() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let script = match &workspace.storage {
            WorkspaceStorage::Script(script) => script,
            WorkspaceStorage::Project => unreachable!(),
        };
        script
            .write_cached_resolution(&LockFile::default())
            .await
            .unwrap();

        for lock_file_usage in [LockFileUsage::Locked, LockFileUsage::Frozen] {
            let error = workspace
                .resolve_lock_file(
                    None,
                    UpdateLockFileOptions {
                        lock_file_usage,
                        no_install: true,
                        ..Default::default()
                    },
                )
                .await
                .err()
                .expect("an adjacent lock file is required");
            assert!(error.to_string().contains("no lock file exists"));
        }
    }

    #[tokio::test]
    async fn adjacent_lock_file_is_authoritative() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let cache_path = workspace.pixi_dir().join(RESOLUTION_CACHE_FILE);
        let resolution = workspace
            .resolve_lock_file(
                None,
                UpdateLockFileOptions {
                    lock_file_usage: LockFileUsage::Update,
                    no_install: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .0;
        resolution
            .as_lock_file()
            .to_path(&workspace.lock_file_path())
            .unwrap();
        drop(resolution);
        fs_err::write(cache_path, "not json").unwrap();

        workspace
            .resolve_lock_file(
                None,
                UpdateLockFileOptions {
                    lock_file_usage: LockFileUsage::Locked,
                    no_install: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn dry_run_does_not_write_the_cache() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let workspace = script_workspace(root.path(), cache.path());
        let cache_path = workspace.pixi_dir().join(RESOLUTION_CACHE_FILE);

        workspace
            .resolve_lock_file(
                None,
                UpdateLockFileOptions {
                    lock_file_usage: LockFileUsage::DryRun,
                    no_install: true,
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert!(!cache_path.exists());
        assert!(!workspace.lock_file_path().exists());
    }
}
