use std::path::{Component, Path, PathBuf};

/// Normalize a path lexically (no filesystem access) and strip redundant segments.
pub(crate) fn normalize_path(path: &Path) -> PathBuf {
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

pub(crate) fn unixify_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_path_collapses_parent_segments() {
        let normalized = normalize_path(Path::new("recipes/../"));
        assert!(normalized.as_os_str().is_empty());
    }
}
