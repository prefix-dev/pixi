use std::path::Path;
use fs_err::tokio as tokio_fs;

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
 
    let target_dir = if std::fs::metadata(dir)?.permissions().readonly() {
        tracing::warn!(
            path = %path.display(),
            "parent directory is read-only; temp file will be created in the system temp dir. \
             Write will not be atomic."
        );
        std::env::temp_dir()
    } else {
        dir.to_path_buf()
    };
 
    tempfile::Builder::new().prefix(&prefix).tempfile_in(target_dir)
}
/// Atomically write contents to a file by first writing to a temporary file and
/// then renaming it to the target path.
///
/// This ensures that the target file is never left in a partially-written state.
/// If the write fails (e.g., due to disk full), the original file remains
/// untouched.
pub async fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let temp_file = temp_file_for(path)?;
    let temp_path = temp_file.into_temp_path();
 
    let contents_ref = contents.as_ref();
    tokio_fs::write(&temp_path, contents_ref).await?;
 
    match temp_path.persist(path) {
        Ok(()) => Ok(()),
        Err(e) if e.error.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(
                path = %path.display(),
                "atomic rename failed due to permissions; falling back to direct write. \
                 Write will not be atomic."
            );
            tokio_fs::write(path, contents_ref).await
        }
        Err(e) => Err(e.error),
    }
}

/// Synchronous version of [`atomic_write`].
pub fn atomic_write_sync(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut temp_file = temp_file_for(path)?;
 
    let contents_ref = contents.as_ref();
    std::io::Write::write_all(&mut temp_file, contents_ref)?;
 
    match temp_file.persist(path) {
        Ok(_) => Ok(()),
        Err(e) if e.error.kind() == std::io::ErrorKind::PermissionDenied => {
            tracing::warn!(
                path = %path.display(),
                "atomic rename failed due to permissions; falling back to direct write. \
                 Write will not be atomic."
            );
            std::fs::write(path, contents_ref)
        }
        Err(e) => Err(e.error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    #[test]
    #[cfg(unix)]
    fn test_temp_file_falls_back_to_tmp_when_parent_not_writable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");
        fs::write(&target, b"[project]").unwrap();

        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

        let temp = temp_file_for(&target).unwrap();

        assert_eq!(temp.path().parent().unwrap(), std::env::temp_dir());

        // resetting the permissions for cleanup
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Integration test: when the parent directory is read-only, `atomic_write`
    /// should fall back to a direct write and the file contents must be correct.
    ///
    /// Note: on Unix, a read-only directory still allows writing to existing
    /// files within it (controlled by the file's own permissions), so the
    /// fallback `tokio_fs::write` succeeds even though rename cannot.
    #[tokio::test]
    #[cfg(unix)]
    async fn test_temp_atomic_write_falls_back_when_dir_not_writable() {
        use std::os::unix::fs::PermissionsExt;
 
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");
        let contents = b"[project]\nname = \"test\"";
 
        fs::write(&target, b"").unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();
 
        atomic_write(&target, contents).await.unwrap();
 
        let written = fs::read(&target).unwrap();
        assert_eq!(written, contents);

        // Reset permissions for clean up
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    }



}
