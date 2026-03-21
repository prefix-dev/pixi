use std::path::Path;

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

    match tempfile::Builder::new().prefix(&prefix).tempfile_in(dir) {
        Ok(file) => Ok(file),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            tempfile::Builder::new()
                .prefix(&prefix)
                .tempfile_in(std::env::temp_dir())
        }
        Err(e) => Err(e),
    }
}

/// Atomically write contents to a file by first writing to a temporary file and
/// then renaming it to the target path.
///
/// This ensures that the target file is never left in a partially-written state.
/// If the write fails (e.g., due to disk full), the original file remains
/// untouched.
pub async fn atomic_write(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    // Create a temp file in the same directory to ensure it's on the same
    // filesystem, which is required for atomic rename.
    let temp_file = temp_file_for(path)?;
    let temp_path = temp_file.into_temp_path();

    // Write contents to the temp file. If this fails (e.g. disk full), the temp
    // file is automatically cleaned up when `temp_path` is dropped.
    tokio::fs::write(&temp_path, contents).await?;

    // Atomically rename the temp file to the target path.
    temp_path.persist(path).map_err(|e| e.error)?;

    Ok(())
}

/// Synchronous version of [`atomic_write`].
pub fn atomic_write_sync(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut temp_file = temp_file_for(path)?;
    std::io::Write::write_all(&mut temp_file, contents.as_ref())?;
    temp_file.persist(path).map_err(|e| e.error)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// temp file in same dir as pixi.toml.
    #[test]
    fn test_temp_file_created_in_same_dir_when_writable() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");

        let temp = temp_file_for(&target).unwrap();

        assert_eq!(temp.path().parent().unwrap(), dir.path());
    }

    /// to test that if the parent dir is not writeable
    /// the temp file is created in $TEMPDIR
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
    /// To test the prefix
    #[test]
    fn temp_file_has_correct_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("pixi.toml");

        let temp = temp_file_for(&target).unwrap();
        let name = temp.path().file_name().unwrap().to_str().unwrap();

        assert!(
            name.starts_with(".pixi.toml."),
            "expected prefix `.pixi.toml.`, got `{name}`"
        );
    }
}