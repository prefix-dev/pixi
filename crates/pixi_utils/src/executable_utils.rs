use std::path::Path;

/// Strips known Windows executable extensions from a file name.
pub(crate) fn strip_windows_executable_extension(file_name: String) -> String {
    let file_name = file_name.to_lowercase();
    // Attempt to retrieve the PATHEXT environment variable
    let extensions_list: Vec<String> = if let Ok(pathext) = std::env::var("PATHEXT") {
        pathext.split(';').map(|s| s.to_lowercase()).collect()
    } else {
        // Fallback to a default list if PATHEXT is not set
        tracing::debug!("Could not find 'PATHEXT' variable, using a default list");
        [
            ".COM", ".EXE", ".BAT", ".CMD", ".VBS", ".VBE", ".JS", ".JSE", ".WSF", ".WSH", ".MSC",
            ".CPL",
        ]
        .iter()
        .map(|s| s.to_lowercase())
        .collect()
    };

    // Attempt to strip any known Windows executable extension
    extensions_list
        .iter()
        .find_map(|ext| file_name.strip_suffix(ext))
        .map(|f| f.to_string())
        .unwrap_or(file_name)
}

/// Strips known Unix executable extensions from a file name.
pub(crate) fn strip_unix_executable_extension(file_name: String) -> String {
    let file_name = file_name.to_lowercase();

    // Define a list of common Unix executable extensions
    let extensions_list: Vec<&str> = vec![
        ".sh", ".bash", ".zsh", ".csh", ".tcsh", ".ksh", ".fish", ".py", ".pl", ".rb", ".lua",
        ".php", ".tcl", ".awk", ".sed",
    ];

    // Attempt to strip any known Unix executable extension
    extensions_list
        .iter()
        .find_map(|&ext| file_name.strip_suffix(ext))
        .map(|f| f.to_string())
        .unwrap_or(file_name)
}

/// Strips known executable extensions from a file name based on the target operating system.
///
/// This function acts as a wrapper that calls either `strip_windows_executable_extension`
/// or `strip_unix_executable_extension` depending on the target OS.
pub fn executable_from_path(path: &Path) -> String {
    let file_name = path
        .iter()
        .last()
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::python312_linux("python3.12", "python3.12")]
    #[case::python3_linux("python3", "python3")]
    #[case::python_linux("python", "python")]
    #[case::python3121_linux("python3.12.1", "python3.12.1")]
    #[case::bash_script("bash.sh", "bash")]
    #[case::zsh59("zsh-5.9", "zsh-5.9")]
    #[case::python_312config("python3.12-config", "python3.12-config")]
    #[case::python3_config("python3-config", "python3-config")]
    #[case::x2to3("2to3", "2to3")]
    #[case::x2to3312("2to3-3.12", "2to3-3.12")]
    #[case::nested_executable("subdir/executable.sh", "subdir/executable")]
    fn test_strip_executable_unix(#[case] path: &str, #[case] expected: &str) {
        let path = Path::new(path);
        let result = strip_unix_executable_extension(path.to_string_lossy().to_string());
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::python_windows("python.exe", "python")]
    #[case::python3_windows("python3.exe", "python3")]
    #[case::python312_windows("python3.12.exe", "python3.12")]
    #[case::package010_windows("package0.1.0.bat", "package0.1.0")]
    #[case::bash("bash", "bash")]
    #[case::zsh59("zsh-5.9", "zsh-5.9")]
    #[case::python_312config("python3.12-config", "python3.12-config")]
    #[case::python3_config("python3-config", "python3-config")]
    #[case::x2to3("2to3", "2to3")]
    #[case::x2to3312("2to3-3.12", "2to3-3.12")]
    #[case::nested_executable("subdir\\executable.exe", "subdir\\executable")]
    fn test_strip_executable_windows(#[case] path: &str, #[case] expected: &str) {
        let path = Path::new(path);
        let result = strip_windows_executable_extension(path.to_string_lossy().to_string());
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::bash("bash", "bash")]
    #[case::zsh59("zsh-5.9", "zsh-5.9")]
    #[case::python_312config("python3.12-config", "python3.12-config")]
    #[case::python3_config("python3-config", "python3-config")]
    #[case::package010("package0.1.0", "package0.1.0")]
    #[case::x2to3("2to3", "2to3")]
    #[case::x2to3312("2to3-3.12", "2to3-3.12")]
    #[case::nested_executable("subdir/executable", "subdir/executable")]
    fn test_strip_executable_extension(#[case] path: &str, #[case] expected: &str) {
        let result = strip_executable_extension(path.into());
        assert_eq!(result, expected);
        // Make sure running it twice doesn't break it
        let result = strip_executable_extension(result);
        assert_eq!(result, expected);
    }
}
