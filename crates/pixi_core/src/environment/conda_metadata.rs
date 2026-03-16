use std::path::Path;

use fs_err as fs;
use miette::IntoDiagnostic;
use pixi_consts::consts;
use std::io;

// Write the contents to the file at the given path.
fn write_file<P: AsRef<Path>, C: AsRef<[u8]>>(path: P, contents: C) -> io::Result<()> {
    // Verify existence of parent
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(path, contents)
}

/// Create the prefix location file.
/// Give it the environment path to place it.
pub fn create_prefix_location_file(environment_dir: &Path) -> miette::Result<()> {
    let prefix_file_path = environment_dir
        .join(consts::CONDA_META_DIR)
        .join(consts::PREFIX_FILE_NAME);

    let parent_dir = prefix_file_path.parent().ok_or_else(|| {
        miette::miette!(
            "Cannot find parent directory of '{}'",
            prefix_file_path.display()
        )
    })?;

    if parent_dir.exists() {
        let contents = parent_dir.to_string_lossy();

        // Read existing contents to determine if an update is necessary
        if prefix_file_path.exists() {
            let existing_contents = fs_err::read_to_string(&prefix_file_path).into_diagnostic()?;
            if existing_contents == contents {
                tracing::info!("No update needed for the prefix file.");
                return Ok(());
            }
        }

        write_file(&prefix_file_path, contents.as_bytes()).into_diagnostic()?;

        tracing::debug!("Prefix file updated with: '{}'.", contents);
    }
    Ok(())
}

/// Create the conda-meta/history.
/// This file is needed for `conda run -p .pixi/envs/<env>` to work.
pub fn create_history_file(environment_dir: &Path) -> miette::Result<()> {
    let history_file = environment_dir.join(consts::CONDA_META_DIR).join("history");

    tracing::debug!("Verify history file exists: {}", history_file.display());

    write_file(
        history_file,
        "// not relevant for pixi but for `conda run -p`",
    )
    .into_diagnostic()
}
