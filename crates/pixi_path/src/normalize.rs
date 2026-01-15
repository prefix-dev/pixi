use typed_path::{
    Utf8Component, Utf8Encoding, Utf8Path, Utf8PathBuf, Utf8TypedPath, Utf8TypedPathBuf,
};

/// A slightly modified version of [`Utf8TypedPath::normalize`] that retains
/// `..` components that lead outside the path.
pub fn normalize_typed(path: Utf8TypedPath<'_>) -> Utf8TypedPathBuf {
    match path {
        Utf8TypedPath::Unix(path) => Utf8TypedPathBuf::Unix(normalize(path)),
        Utf8TypedPath::Windows(path) => Utf8TypedPathBuf::Windows(normalize(path)),
    }
}

/// A slightly modified version of [`Utf8Path::normalize`] that retains `..`
/// components that lead outside the path.
fn normalize<T: Utf8Encoding>(path: &Utf8Path<T>) -> Utf8PathBuf<T> {
    let mut components = Vec::new();
    for component in path.components() {
        if !component.is_current() && !component.is_parent() {
            components.push(component);
        } else if component.is_parent() {
            if let Some(last) = components.last() {
                if last.is_normal() {
                    components.pop();
                } else {
                    components.push(component);
                }
            } else {
                components.push(component);
            }
        }
    }

    let mut path = Utf8PathBuf::<T>::new();

    for component in components {
        path.push(component.as_str());
    }

    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use typed_path::Utf8TypedPath;

    #[test]
    fn test_normalize_collapses_parent_in_middle() {
        // a/b/../c -> a/c
        let path = Utf8TypedPath::derive("a/b/../c");
        let normalized = normalize_typed(path);
        assert_eq!(normalized.as_str(), "a/c");
    }

    #[test]
    fn test_normalize_retains_leading_parent() {
        // ../a -> ../a (cannot collapse leading ..)
        let path = Utf8TypedPath::derive("../a");
        let normalized = normalize_typed(path);
        assert_eq!(normalized.as_str(), "../a");
    }

    #[test]
    fn test_normalize_retains_multiple_leading_parents() {
        // ../../a/b -> ../../a/b
        let path = Utf8TypedPath::derive("../../a/b");
        let normalized = normalize_typed(path);
        assert_eq!(normalized.as_str(), "../../a/b");
    }

    #[test]
    fn test_normalize_collapses_current_dir() {
        // a/./b -> a/b
        let path = Utf8TypedPath::derive("a/./b");
        let normalized = normalize_typed(path);
        assert_eq!(normalized.as_str(), "a/b");
    }

    #[test]
    fn test_normalize_complex_path() {
        // a/b/c/../../d -> a/d
        let path = Utf8TypedPath::derive("a/b/c/../../d");
        let normalized = normalize_typed(path);
        assert_eq!(normalized.as_str(), "a/d");
    }

    #[test]
    fn test_normalize_parent_escapes_base() {
        // a/../.. -> .. (goes outside the base)
        let path = Utf8TypedPath::derive("a/../..");
        let normalized = normalize_typed(path);
        assert_eq!(normalized.as_str(), "..");
    }

    #[test]
    fn test_normalize_relative_going_up_then_down() {
        // ../sibling/subdir -> ../sibling/subdir
        let path = Utf8TypedPath::derive("../sibling/subdir");
        let normalized = normalize_typed(path);
        assert_eq!(normalized.as_str(), "../sibling/subdir");
    }
}
