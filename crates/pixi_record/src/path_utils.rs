use std::path::{Path, PathBuf};

/// Normalize a path lexically (no filesystem access) and strip redundant segments.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
    use std::path::Component;

    let simplified = dunce::simplified(path).to_path_buf();

    let mut prefix: Option<std::ffi::OsString> = None;
    let mut has_root = false;
    let mut parts: Vec<std::ffi::OsString> = Vec::new();

    for component in simplified.components() {
        match component {
            Component::Prefix(prefix_component) => {
                prefix = Some(prefix_component.as_os_str().to_os_string());
                parts.clear();
            }
            Component::RootDir => {
                has_root = true;
                parts.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = parts.last() {
                    if last.as_os_str() == std::ffi::OsStr::new("..") {
                        parts.push(std::ffi::OsString::from(".."));
                    } else {
                        parts.pop();
                    }
                } else if !has_root {
                    parts.push(std::ffi::OsString::from(".."));
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
        }
    }

    let mut normalized = PathBuf::new();
    if let Some(prefix) = prefix {
        normalized.push(prefix);
    }
    if has_root {
        normalized.push(std::path::MAIN_SEPARATOR.to_string());
    }
    for part in parts {
        normalized.push(part);
    }

    normalized
}

/// Compute the repo-relative path from `base` (manifest subdir) to `target` (build subdir), always
/// returning `/`-separated strings so the lock format is stable across platforms.
pub(crate) fn relative_repo_subdir(base: &str, target: &str) -> Option<String> {
    let base_abs = repo_absolute_path(base);
    let target_abs = repo_absolute_path(target);
    let relative =
        pathdiff::diff_paths(&target_abs, &base_abs).unwrap_or_else(|| target_abs.clone());
    let normalized = normalize_path(&relative);
    if normalized.as_os_str().is_empty() {
        None
    } else {
        Some(unixify_path(&normalized))
    }
}

/// Apply a manifest subdir back onto a relative path stored in the lock, again emitting `/`.
pub(crate) fn resolve_repo_subdir(base: &str, relative: Option<&str>) -> Option<String> {
    match relative {
        Some(rel) => {
            let combined = repo_absolute_path(base).join(rel);
            strip_repo_root(normalize_path(&combined))
        }
        None => {
            if base.is_empty() {
                None
            } else {
                Some(unixify_str(base))
            }
        }
    }
}

fn repo_absolute_path(subdir: &str) -> PathBuf {
    if subdir.is_empty() {
        PathBuf::from("/")
    } else {
        Path::new("/").join(subdir)
    }
}

fn strip_repo_root(path: PathBuf) -> Option<String> {
    let trimmed = if path.has_root() {
        match path.strip_prefix(Path::new("/")) {
            Ok(stripped) => stripped.to_path_buf(),
            Err(_) => path,
        }
    } else {
        path
    };

    if trimmed.as_os_str().is_empty() {
        None
    } else {
        Some(unixify_path(&trimmed))
    }
}

pub(crate) fn unixify_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(crate) fn unixify_str(value: &str) -> String {
    value.replace('\\', "/")
}

pub(crate) fn path_within_workspace(
    path_str: &str,
    native_path: &Path,
    workspace_root: &Path,
) -> bool {
    if native_path.is_absolute() {
        return native_path.starts_with(workspace_root);
    }

    let path_unix = unixify_str(path_str);
    let mut workspace_unix = unixify_path(workspace_root);
    if workspace_unix.ends_with('/') {
        workspace_unix.pop();
    }

    if workspace_unix.is_empty() {
        return path_unix.starts_with('/');
    }

    if path_unix == workspace_unix {
        return true;
    }

    path_unix.len() > workspace_unix.len()
        && path_unix.starts_with(&workspace_unix)
        && path_unix.as_bytes()[workspace_unix.len()] == b'/'
}

/// Returns true if the string should be treated as absolute regardless of host OS.
pub(crate) fn is_cross_platform_absolute(path_str: &str, native_path: &Path) -> bool {
    if native_path.is_absolute() {
        return true;
    }

    if path_str.starts_with('/') || path_str.starts_with('\\') {
        return true;
    }

    if path_str.len() >= 3 {
        let bytes = path_str.as_bytes();
        let drive = bytes[0];
        let colon = bytes[1];
        let slash = bytes[2];

        if drive.is_ascii_alphabetic() && colon == b':' && (slash == b'/' || slash == b'\\') {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_collapses_parent_segments() {
        let normalized = normalize_path(Path::new("recipes/../"));
        assert!(normalized.as_os_str().is_empty());
    }

    #[test]
    fn repo_subdir_helpers_round_trip() {
        let manifest = "recipes";
        let build = "recipes/lib";

        let relative = relative_repo_subdir(manifest, build).expect("should produce relative path");
        assert_eq!(relative, "lib");

        let resolved = resolve_repo_subdir(manifest, Some(relative.as_str()));
        assert_eq!(resolved.as_deref(), Some("recipes/lib"));
    }

    #[test]
    fn repo_subdir_helpers_handle_root() {
        assert!(relative_repo_subdir("", "").is_none());
        assert!(resolve_repo_subdir("", None).is_none());

        let rel = relative_repo_subdir("", "src").expect("relative path");
        assert_eq!(rel, "src");
        assert_eq!(resolve_repo_subdir("", Some("src")).as_deref(), Some("src"));
    }

    #[test]
    fn cross_platform_absolute_detection() {
        assert!(is_cross_platform_absolute(
            "/opt/external",
            Path::new("/opt/external")
        ));
        assert!(is_cross_platform_absolute(
            r"C:\work",
            Path::new(r"C:\work")
        ));
        assert!(is_cross_platform_absolute(
            r"\\server\share",
            Path::new(r"\\server\share")
        ));
        assert!(is_cross_platform_absolute(
            "/just/slash",
            Path::new("just\\slash")
        ));
        assert!(!is_cross_platform_absolute(
            "relative/path",
            Path::new("relative/path")
        ));
    }

    #[test]
    fn path_within_workspace_detection() {
        assert!(path_within_workspace(
            "/workspace/src",
            Path::new("/workspace/src"),
            Path::new("/workspace")
        ));

        assert!(path_within_workspace(
            "/workspace/src",
            Path::new("workspace\\src"),
            Path::new("/workspace")
        ));

        assert!(!path_within_workspace(
            "/other/src",
            Path::new("/other/src"),
            Path::new("/workspace")
        ));
    }
}
