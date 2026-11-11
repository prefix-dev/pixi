use std::path::{Component, Path, PathBuf};

use typed_path::{
    Utf8Component, Utf8Components, Utf8Encoding, Utf8Path, Utf8PathBuf, Utf8TypedPath,
    Utf8TypedPathBuf, Utf8UnixComponent, Utf8WindowsComponent,
};

/// Small adapter trait for components that can participate in lexical path normalization.
///
/// Both [`std::path::Component`] and `typed_path` UTF-8 components expose the
/// same conceptual component kinds, but through different APIs.
trait LexicalComponent {
    fn is_current(&self) -> bool;
    fn is_parent(&self) -> bool;
    fn is_normal(&self) -> bool;
    fn is_root(&self) -> bool;
}

impl LexicalComponent for Component<'_> {
    fn is_current(&self) -> bool {
        matches!(self, Component::CurDir)
    }

    fn is_parent(&self) -> bool {
        matches!(self, Component::ParentDir)
    }

    fn is_normal(&self) -> bool {
        matches!(self, Component::Normal(_))
    }

    fn is_root(&self) -> bool {
        matches!(self, Component::RootDir)
    }
}

impl LexicalComponent for Utf8UnixComponent<'_> {
    fn is_current(&self) -> bool {
        Utf8Component::is_current(self)
    }

    fn is_parent(&self) -> bool {
        Utf8Component::is_parent(self)
    }

    fn is_normal(&self) -> bool {
        Utf8Component::is_normal(self)
    }

    fn is_root(&self) -> bool {
        Utf8Component::is_root(self)
    }
}

impl LexicalComponent for Utf8WindowsComponent<'_> {
    fn is_current(&self) -> bool {
        Utf8Component::is_current(self)
    }

    fn is_parent(&self) -> bool {
        Utf8Component::is_parent(self)
    }

    fn is_normal(&self) -> bool {
        Utf8Component::is_normal(self)
    }

    fn is_root(&self) -> bool {
        Utf8Component::is_root(self)
    }
}

/// Normalize component stacks by removing current-directory components and
/// resolving parent-directory components where possible.
fn normalize_component_stack<C: LexicalComponent>(
    components: impl IntoIterator<Item = C>,
) -> Vec<C> {
    let mut out: Vec<C> = Vec::new();
    for component in components {
        if component.is_current() {
            continue;
        }

        if component.is_parent() {
            // Pop the last normal component if present, drop if at root,
            // otherwise keep the ParentDir.
            match out.last() {
                Some(last) if last.is_normal() => {
                    out.pop();
                }
                Some(last) if last.is_root() => {
                    // Can't go above root directory; ignore this ParentDir.
                }
                _ => {
                    out.push(component);
                }
            }
        } else {
            out.push(component);
        }
    }
    out
}

/// Lexically normalize a path without accessing the filesystem.
///
/// This removes `.` components and resolves `..` components where possible.
/// It preserves leading `..` components for relative paths and does not follow
/// symlinks or require the path to exist.
pub fn normalize_std(path: &Path) -> PathBuf {
    normalize_component_stack(path.components())
        .iter()
        .collect()
}

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
fn normalize<T: Utf8Encoding>(path: &Utf8Path<T>) -> Utf8PathBuf<T>
where
    for<'a> <T::Components<'a> as Utf8Components<'a>>::Component: LexicalComponent,
{
    // This is the actual normalization logic.
    let components = normalize_component_stack(path.components());

    components
        .into_iter()
        .map(|component| Utf8Path::<T>::new(component.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use typed_path::Utf8TypedPath;

    #[test]
    fn test_normalize_std() {
        assert_eq!(
            normalize_std(std::path::Path::new("./.././.././")),
            std::path::Path::new("../..")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("recipe/../")),
            std::path::Path::new("")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("foo/bar/../baz")),
            std::path::Path::new("foo/baz")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("../recipe/..")),
            std::path::Path::new("..")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("/..")),
            std::path::Path::new("/")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("/../foo")),
            std::path::Path::new("/foo")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("/foo/..")),
            std::path::Path::new("/")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("/.")),
            std::path::Path::new("/")
        );
        assert_eq!(
            normalize_std(std::path::Path::new("/foo/bar/../..")),
            std::path::Path::new("/")
        );
    }

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
