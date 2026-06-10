//! Support for ament_python packages that ship a PEP 621 `pyproject.toml`
//! instead of a `setup.py`.
//!
//! Such packages cannot register themselves in the ament resource index
//! (setup.py packages do this through `data_files`), so the build script has
//! to create the index entries and expose `[project.scripts]` entry points in
//! `lib/<package>` where `ros2 run` looks for executables.

use std::path::Path;

use miette::Diagnostic;
use pyproject_toml::PyProjectToml;
use thiserror::Error;

/// Errors that can occur while inspecting a `pyproject.toml`.
#[derive(Debug, Error, Diagnostic)]
pub enum AmentPythonError {
    #[error("failed to read {path}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

/// How an ament_python package describes its Python build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmentPythonFlavor {
    /// The package has a `setup.py` (or neither file, in which case the
    /// setup.py based build script is kept as the historical fallback).
    SetupPy,
    /// The package only has a `pyproject.toml`.
    Pyproject,
}

/// Detect the build flavor of an ament_python package. `setup.py` wins when
/// both files exist; many setup.py packages carry a `pyproject.toml` purely
/// for tool configuration.
pub fn detect_flavor(source_dir: &Path) -> AmentPythonFlavor {
    if !source_dir.join("setup.py").is_file() && source_dir.join("pyproject.toml").is_file() {
        AmentPythonFlavor::Pyproject
    } else {
        AmentPythonFlavor::SetupPy
    }
}

/// Collect the entry point names from `[project.scripts]` and
/// `[project.gui-scripts]` of a `pyproject.toml`.
pub fn entry_points(pyproject_path: &Path) -> Result<Vec<String>, AmentPythonError> {
    let content = fs_err::read_to_string(pyproject_path).map_err(|err| AmentPythonError::Io {
        path: pyproject_path.display().to_string(),
        source: err,
    })?;
    let manifest: PyProjectToml =
        toml::from_str(&content).map_err(|err| AmentPythonError::Parse {
            path: pyproject_path.display().to_string(),
            source: err,
        })?;

    let mut scripts = Vec::new();
    if let Some(project) = manifest.project {
        scripts.extend(project.scripts.into_iter().flatten().map(|(name, _)| name));
        scripts.extend(
            project
                .gui_scripts
                .into_iter()
                .flatten()
                .map(|(name, _)| name),
        );
    }
    Ok(scripts)
}

/// Generate the build script lines that copy the installed entry points into
/// `lib/<package>` so `ros2 run` can find them. pip only installs them into
/// `bin` (`Scripts` on Windows). Returns an empty string when there are no
/// entry points.
pub fn entry_point_install_lines(
    package_name: &str,
    scripts: &[String],
    is_windows: bool,
) -> String {
    if scripts.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    if is_windows {
        lines.push(format!(
            ":: Make console scripts discoverable by `ros2 run` (it looks in lib\\{package_name})"
        ));
        lines.push(format!(
            "if not exist \"%LIBRARY_PREFIX%\\lib\\{package_name}\" mkdir \"%LIBRARY_PREFIX%\\lib\\{package_name}\""
        ));
        for script in scripts {
            // Wildcard to catch both the `.exe` and `-script.py` wrappers.
            lines.push(format!(
                "copy \"%PREFIX%\\Scripts\\{script}*\" \"%LIBRARY_PREFIX%\\lib\\{package_name}\\\""
            ));
        }
        lines.push("if errorlevel 1 exit 1".to_string());
    } else {
        lines.push(format!(
            "# Make console scripts discoverable by `ros2 run` (it looks in lib/{package_name})"
        ));
        lines.push(format!("mkdir -p \"$PREFIX/lib/{package_name}\""));
        for script in scripts {
            lines.push(format!(
                "cp \"$PREFIX/bin/{script}\" \"$PREFIX/lib/{package_name}/{script}\""
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PYPROJECT: &str = r#"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"

[project]
name = "navigator"
version = "0.1.0"

[project.scripts]
navigate = "navigator.navigator:main"

[project.gui-scripts]
navigate-gui = "navigator.gui:main"
"#;

    fn write_pyproject(dir: &Path) {
        fs_err::write(dir.join("pyproject.toml"), PYPROJECT).unwrap();
    }

    #[test]
    fn test_detect_flavor_setup_py_only() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join("setup.py"), "").unwrap();
        assert_eq!(detect_flavor(dir.path()), AmentPythonFlavor::SetupPy);
    }

    #[test]
    fn test_detect_flavor_pyproject_only() {
        let dir = tempfile::tempdir().unwrap();
        write_pyproject(dir.path());
        assert_eq!(detect_flavor(dir.path()), AmentPythonFlavor::Pyproject);
    }

    #[test]
    fn test_detect_flavor_setup_py_wins_over_pyproject() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join("setup.py"), "").unwrap();
        write_pyproject(dir.path());
        assert_eq!(detect_flavor(dir.path()), AmentPythonFlavor::SetupPy);
    }

    #[test]
    fn test_detect_flavor_neither_falls_back_to_setup_py() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_flavor(dir.path()), AmentPythonFlavor::SetupPy);
    }

    #[test]
    fn test_entry_points() {
        let dir = tempfile::tempdir().unwrap();
        write_pyproject(dir.path());
        let scripts = entry_points(&dir.path().join("pyproject.toml")).unwrap();
        assert_eq!(scripts, vec!["navigate", "navigate-gui"]);
    }

    #[test]
    fn test_entry_points_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = entry_points(&dir.path().join("pyproject.toml"));
        assert!(matches!(result, Err(AmentPythonError::Io { .. })));
    }

    #[test]
    fn test_entry_points_invalid_toml() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join("pyproject.toml"), "not = [valid").unwrap();
        let result = entry_points(&dir.path().join("pyproject.toml"));
        assert!(matches!(result, Err(AmentPythonError::Parse { .. })));
    }

    #[test]
    fn test_entry_point_install_lines_unix() {
        let lines = entry_point_install_lines("navigator", &["navigate".to_string()], false);
        assert!(lines.contains("mkdir -p \"$PREFIX/lib/navigator\""));
        assert!(lines.contains("cp \"$PREFIX/bin/navigate\" \"$PREFIX/lib/navigator/navigate\""));
    }

    #[test]
    fn test_entry_point_install_lines_windows() {
        let lines = entry_point_install_lines("navigator", &["navigate".to_string()], true);
        assert!(lines.contains("%LIBRARY_PREFIX%\\lib\\navigator"));
        assert!(lines.contains("copy \"%PREFIX%\\Scripts\\navigate*\""));
    }

    #[test]
    fn test_entry_point_install_lines_empty() {
        assert_eq!(entry_point_install_lines("navigator", &[], false), "");
        assert_eq!(entry_point_install_lines("navigator", &[], true), "");
    }
}
