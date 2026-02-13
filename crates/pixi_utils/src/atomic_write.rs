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

/// Atomically write contents to a file by first writing to a temporary file in
/// the same directory and then renaming it to the target path.
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
