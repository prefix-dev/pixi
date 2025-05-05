use std::{ffi::OsStr, path::Path, sync::LazyLock};

/// Returns the name of the binary. Since this is used in errors, it resolves to "pixi" if it can't
/// find the resolve name rather than return an error itself.
pub fn executable_name() -> &'static str {
    pub static PIXI_BIN_NAME: LazyLock<String> = LazyLock::new(|| {
        std::env::args()
            .next()
            .as_ref()
            .map(Path::new)
            .and_then(Path::file_stem)
            .and_then(OsStr::to_str)
            .map(String::from)
            .unwrap_or("pixi".to_string())
    });
    PIXI_BIN_NAME.as_str()
}

/// Strips known Windows executable extensions from a file name.
pub(crate) fn strip_windows_executable_extension(file_name: String) -> String {
    // Create lowercase version for comparison
    let lowercase_name = file_name.to_lowercase();

    // Get extensions list
    let extensions_list: Vec<String> = if let Ok(pathext) = std::env::var("PATHEXT") {
        pathext.split(';').map(|s| s.to_lowercase()).collect()
    } else {
        tracing::debug!("Could not find 'PATHEXT' variable, using a default list");
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

/// Checks if the given relative path points to an identified binary folder.
pub fn is_binary_folder(relative_path: &Path) -> bool {
    // Check if the file is in a known executable directory.
    let binary_folders = if cfg!(windows) {
        &([
            "",
            "Library/mingw-w64/bin/",
            "Library/usr/bin/",
            "Library/bin/",
            "Scripts/",
            "bin/",
        ][..])
    } else {
        &(["bin"][..])
    };
    binary_folders
        .iter()
        .any(|bin_path| Path::new(bin_path) == relative_path)
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

    #[test]
    fn test_is_binary_folder() {
        let path = Path::new("subdir");
        let result = is_binary_folder(path);
        assert!(!result);

        let path = Path::new("bin");
        let result = is_binary_folder(path);
        assert!(result);

        let path = Path::new("Library/bin/");
        let result = is_binary_folder(path);
        if cfg!(windows) {
            assert!(result);
        } else {
            assert!(!result);
        }
    }

    #[test]
    fn test_unix_extensions() {
        let test_cases = vec![
            ("script.sh", "script"),
            ("MyScript.PY", "MyScript"),
            ("CamelCase.Sh", "CamelCase"),
            ("noextension", "noextension"),
            ("", ""),
            ("path/to/Script.bash", "path/to/Script"),
            ("my.script.py", "my.script"),
        ];

        for (input, expected) in test_cases {
            assert_eq!(strip_unix_executable_extension(input.to_string()), expected);
        }
    }

    #[test]
    fn test_windows_extensions() {
        let test_cases = vec![
            ("program.exe", "program"),
            ("MyProgram.EXE", "MyProgram"),
            ("Script.BAT", "Script"),
            ("noextension", "noextension"),
            ("", ""),
            ("C:\\Path\\To\\Program.COM", "C:\\Path\\To\\Program"),
            ("my.program.exe", "my.program"),
            ("WeIrDcAsE.ExE", "WeIrDcAsE"),
        ];

        for (input, expected) in test_cases {
            assert_eq!(
                strip_windows_executable_extension(input.to_string()),
                expected
            );
        }
    }
}

