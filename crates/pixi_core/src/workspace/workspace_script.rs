use std::path::{Path, PathBuf};

use pixi_manifest::script::ScriptManifest;
use xxhash_rust::xxh3::xxh3_64;

/// Describes where a script reads and writes workspace state.
///
/// Every script has a parsed manifest and a cache directory. A local script
/// also has an adjacent lock-file path. A transient script (e.g., one read from
/// a URL or standard input) does not have one.
#[derive(Debug, Clone)]
pub(super) struct WorkspaceScript {
    manifest: Box<ScriptManifest>,
    pixi_dir: PathBuf,

    /// The adjacent lock-file path for a local script.
    lock_file_path: Option<PathBuf>,
}

impl WorkspaceScript {
    pub(super) fn for_local(manifest: ScriptManifest, cache_root: &Path) -> Self {
        let pixi_dir = cache_root.join(local_cache_name(manifest.path()));
        let lock_file_path = Some(local_lock_file_path(manifest.path()));
        Self {
            manifest: Box::new(manifest),
            pixi_dir,
            lock_file_path,
        }
    }

    pub(super) fn for_transient(
        manifest: ScriptManifest,
        cache_root: &Path,
        cache_name: &str,
        cache_key: &[u8],
    ) -> Self {
        Self {
            manifest: Box::new(manifest),
            pixi_dir: cache_root.join(transient_cache_name(cache_name, cache_key)),
            lock_file_path: None,
        }
    }

    pub(super) fn manifest(&self) -> &ScriptManifest {
        &self.manifest
    }

    pub(super) fn replace_manifest(&mut self, new_manifest: ScriptManifest) {
        if self.lock_file_path.is_some() {
            self.lock_file_path = Some(local_lock_file_path(new_manifest.path()));
        }
        *self.manifest = new_manifest;
    }

    pub(super) fn pixi_dir(&self) -> &Path {
        &self.pixi_dir
    }

    pub(super) fn lock_file_path(&self) -> Option<PathBuf> {
        self.lock_file_path.clone()
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

fn transient_cache_name(name: &str, key: &[u8]) -> String {
    let digest = format!("{:016x}", xxh3_64(key));
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
    use super::*;

    const SOURCE: &str = "# /// script\n# dependencies = []\n# ///\n";

    fn manifest(path: &Path) -> ScriptManifest {
        ScriptManifest::from_source(path, SOURCE.as_bytes())
            .unwrap()
            .unwrap()
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
        let key = b"first key";
        let digest = xxh3_64(key);
        let first = WorkspaceScript::for_transient(
            manifest(&script_path),
            &cache_root,
            "HTTPS://example.com/example.py",
            key,
        );
        let first_again = WorkspaceScript::for_transient(
            manifest(&script_path),
            &cache_root,
            "HTTPS://example.com/example.py",
            key,
        );
        let second = WorkspaceScript::for_transient(
            manifest(&script_path),
            &cache_root,
            "HTTPS://example.com/example.py",
            b"second key",
        );

        assert!(first.lock_file_path().is_none());
        assert_eq!(
            cache_root.join(format!("https---example-com-example-py-{digest:016x}")),
            first.pixi_dir(),
        );
        assert_eq!(first.pixi_dir(), first_again.pixi_dir());
        assert_ne!(first.pixi_dir(), second.pixi_dir());
    }

    #[test]
    fn empty_transient_cache_name_uses_the_digest() {
        let directory = tempfile::tempdir().unwrap();
        let cache_root = directory.path().join("cache");
        let script_path = directory.path().join("example.py");
        let key = b"cache key";
        let script =
            WorkspaceScript::for_transient(manifest(&script_path), &cache_root, "://", key);
        let expected = cache_root.join(format!("{:016x}", xxh3_64(key)));

        assert_eq!(expected, script.pixi_dir());
    }
}
