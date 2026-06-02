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
/// Atomically write contents to a file by first writing to a temporary file and
/// then renaming it to the target path.
///
/// This ensures that the target file is never left in a partially-written state.
/// If the write fails (e.g., due to disk full), the original file remains
/// untouched.
pub async fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
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
    temp_path.persist(path).map_err(|e| e.error)?;

    Ok(())
}

/// Synchronous version of [`atomic_write`].
pub fn atomic_write_sync(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
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
}
