//! Module ported from pixi_utils to avoid pulling in a lot of dependencies

use std::path::Path;

/// Strips known Windows executable extensions from a file name.
pub(crate) fn strip_windows_executable_extension(file_name: String) -> String {
    // Create lowercase version for comparison
    let lowercase_name = file_name.to_lowercase();

    // Get extensions list
    let extensions_list: Vec<String> = if let Ok(pathext) = std::env::var("PATHEXT") {
        pathext.split(';').map(|s| s.to_lowercase()).collect()
    } else {
        if cfg!(debug_assertions) {
            eprintln!("Could not find 'PATHEXT' variable, using a default list");
        }
        [
            ".com", ".exe", ".bat", ".cmd", ".vbs", ".vbe", ".js", ".jse", ".wsf", ".wsh", ".msc",
            ".cpl",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    };

    // Check for matches while preserving original case
    for ext in extensions_list {
        if lowercase_name.ends_with(&ext) {
            return file_name[..file_name.len() - ext.len()].to_string();
        }
    }

    file_name
}

/// Strips known Unix executable extensions from a file name.
pub(crate) fn strip_unix_executable_extension(file_name: String) -> String {
    // Define a list of common Unix executable extensions
    let extensions_list: Vec<&str> = vec![
        ".sh", ".bash", ".zsh", ".csh", ".tcsh", ".ksh", ".fish", ".py", ".pl", ".rb", ".lua",
        ".php", ".tcl", ".awk", ".sed",
    ];

    // Create lowercase version for comparison only
    let lowercase_name = file_name.to_lowercase();

    // Attempt to strip any known Unix executable extension
    for ext in extensions_list {
        if lowercase_name.ends_with(ext) {
            return file_name[..file_name.len() - ext.len()].to_string();
        }
    }

    file_name
}

/// Strips known executable extensions from a file name based on the target operating system.
///
/// This function acts as a wrapper that calls either `strip_windows_executable_extension`
/// or `strip_unix_executable_extension` depending on the target OS.
pub fn executable_from_path(path: &Path) -> String {
    let file_name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .to_string();
    strip_executable_extension(file_name)
}

/// Strips known executable extensions from a file name based on the target operating system.
pub fn strip_executable_extension(file_name: String) -> String {
    if cfg!(target_family = "windows") {
        strip_windows_executable_extension(file_name)
    } else {
        strip_unix_executable_extension(file_name)
    }
}