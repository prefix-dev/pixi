use fs_err::tokio as tokio_fs;
use std::path::Path;

/// Build a [`tempfile::NamedTempFile`] in the same directory as `path`, using
/// the original filename as the prefix so the temp file is easily identifiable
/// (e.g. `.pixi.toml.XXXXXX`).
fn temp_file_for(path: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    let dir = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path has no parent directory",
        )
    })?;

    let prefix = format!(
        ".{}.",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("tmp")
    );

    tempfile::Builder::new().prefix(&prefix).tempfile_in(dir)
}

/// On Unix, return the permissions of an existing file at `path`, or `None` if
/// the file does not exist.
///
/// This is read *before* the atomic rename so that the original mode is
/// restored on the replacement file.  `tempfile` creates temp files with
/// `0o600`; without this step every atomic rewrite would silently downgrade
/// the destination's permissions.
#[cfg(unix)]
fn original_permissions(path: &Path) -> std::io::Result<Option<std::fs::Permissions>> {
    match fs_err::metadata(path) {
        Ok(m) => Ok(Some(m.permissions())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Atomically write contents to a file by first writing to a temporary file and
/// then renaming it to the target path.
///
/// This ensures that the target file is never left in a partially-written state.
/// If the write fails (e.g., due to disk full), the original file remains
/// untouched.
///
/// On Unix the permissions of the existing file are preserved across the
/// rewrite.  `tempfile` creates temp files with the restrictive `0o600` mode;
/// without the explicit restore step the rename would silently change the
/// destination's mode on every write.
pub async fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    // Read the original permissions before touching anything so we can
    // restore them after the rename.
    #[cfg(unix)]
    let perms = original_permissions(path)?;

    let temp_file = match temp_file_for(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(
                path = %path.display(),
                "cannot create temp file in parent directory; falling back to direct write. \
                Write will not be atomic."
            );
            return tokio_fs::write(path, contents.as_ref()).await;
        }
        Err(e) => return Err(e),
    };

    let temp_path = temp_file.into_temp_path();
    tokio_fs::write(&temp_path, contents.as_ref()).await?;

    // Restore the original file's permissions on the temp file before
    // renaming so the atomic swap never changes the destination's mode.
    #[cfg(unix)]
    if let Some(p) = perms {
        tokio_fs::set_permissions(&temp_path, p).await?;
    }

    temp_path.persist(path).map_err(|e| e.error)?;

    Ok(())
}

/// Synchronous version of [`atomic_write`].
pub fn atomic_write_sync(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    // Read the original permissions before touching anything.
    #[cfg(unix)]
    let perms = original_permissions(path)?;

    let mut temp_file = match temp_file_for(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(
                path = %path.display(),
                "cannot create temp file in parent directory; falling back to direct write. \
                Write will not be atomic."
            );
            return fs_err::write(path, contents.as_ref());
        }
        Err(e) => return Err(e),
    };
    std::io::Write::write_all(&mut temp_file, contents.as_ref())?;

    // Restore the original file's permissions before renaming.
    #[cfg(unix)]
    if let Some(p) = perms {
        fs_err::set_permissions(temp_file.path(), p)?;
    }

    temp_file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temp_file_created_in_same_dir_when_writable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");

        let temp = temp_file_for(&target).unwrap();

        assert_eq!(temp.path().parent().unwrap(), dir.path());
    }

    #[test]
    fn test_temp_file_has_correct_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");

        let temp = temp_file_for(&target).unwrap();
        let name = temp.path().file_name().unwrap().to_str().unwrap();

        assert!(
            name.starts_with(".pixi.toml."),
            "expected prefix `.pixi.toml.`, got `{name}`"
        );
    }

    /// Integration test: when the parent directory is read-only, `atomic_write`
    /// should fall back to a direct write and the file contents must be correct.
    ///
    /// Note: on Unix, a read-only directory still allows writing to existing
    /// files within it (controlled by the file's own permissions), so the
    /// fallback `tokio_fs::write` succeeds even though rename cannot.
    #[tokio::test]
    #[cfg(unix)]
    async fn test_atomic_write_falls_back() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");
        let contents = b"[project]\nname = \"test\"";

        tokio_fs::write(&target, b"").await.unwrap();
        tokio_fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555))
            .await
            .unwrap();

        atomic_write(&target, contents).await.unwrap();

        let written = tokio_fs::read(&target).await.unwrap();
        assert_eq!(written, contents);

        // Reset permissions for clean up
        tokio_fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .await
            .unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn test_atomic_write_sync_falls_back() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");
        let contents = b"[project]\nname = \"test\"";

        fs_err::write(&target, b"").unwrap();
        fs_err::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        atomic_write_sync(&target, contents).unwrap();

        let written = fs_err::read(&target).unwrap();
        assert_eq!(written, contents);

        // Reset permissions for clean up
        fs_err::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// `atomic_write` must not change the mode of an existing file.
    /// This is the regression test for https://github.com/prefix-dev/pixi/issues/6295 —
    /// `project version set` was silently downgrading pixi.toml from 0644 → 0600.
    #[tokio::test]
    #[cfg(unix)]
    async fn test_atomic_write_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");
        let original = b"[workspace]\nversion = \"1.0.0\"\n";
        let updated = b"[workspace]\nversion = \"1.2.3\"\n";

        // Create file with explicit 0o644 permissions (world-readable).
        tokio_fs::write(&target, original).await.unwrap();
        tokio_fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644))
            .await
            .unwrap();

        atomic_write(&target, updated).await.unwrap();

        let mode = tokio_fs::metadata(&target)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o644,
            "atomic_write must not change file permissions (got {mode:#o})"
        );
        assert_eq!(tokio_fs::read(&target).await.unwrap(), updated);
    }

    /// Same regression test for the synchronous path.
    #[test]
    #[cfg(unix)]
    fn test_atomic_write_sync_preserves_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");
        let original = b"[workspace]\nversion = \"1.0.0\"\n";
        let updated = b"[workspace]\nversion = \"1.2.3\"\n";

        fs_err::write(&target, original).unwrap();
        fs_err::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();

        atomic_write_sync(&target, updated).unwrap();

        let mode = fs_err::metadata(&target).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o644,
            "atomic_write_sync must not change file permissions (got {mode:#o})"
        );
        assert_eq!(fs_err::read(&target).unwrap(), updated);
    }

    /// Verify that non-standard permissions (e.g. 0o600) on existing files are
    /// also faithfully preserved — atomic_write must not normalise them to 0644.
    #[tokio::test]
    #[cfg(unix)]
    async fn test_atomic_write_preserves_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");

        tokio_fs::write(&target, b"original\n").await.unwrap();
        tokio_fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
            .await
            .unwrap();

        atomic_write(&target, b"updated\n").await.unwrap();

        let mode = tokio_fs::metadata(&target)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "atomic_write must preserve 0o600 when that was the original mode (got {mode:#o})"
        );
    }
}
