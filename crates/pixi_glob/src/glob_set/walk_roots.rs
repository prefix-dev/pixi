//! Module to determine where to start globbing, by splitting the `..`, and `.` components from a glob
//! This will then determine what we need to join with the current glob path to start globbing from that location.
//! We need to split the effective walk roots, so that we can do a single globbing pass from there, which
//! is probably the best heuristic to be efficient

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

/// A non-processed glob so its easier to split into negated
/// an non-negated later on
struct SimpleGlob {
    /// Glob to use
    pub glob: String,
    /// Determine if its negated, a !glob
    pub negated: bool,
}

impl SimpleGlob {
    pub fn new(glob: String, negated: bool) -> Self {
        Self { glob, negated }
    }

    pub fn pattern(&self) -> &str {
        &self.glob
    }

    pub fn is_negated(&self) -> bool {
        self.negated
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WalkRootsError {
    #[error("after processing glob '{glob}', split into '{prefix}' and empty glob")]
    EmptyGlob { prefix: String, glob: String },
}

/// Contains the globs and the joinable path
pub struct WalkRoots {
    roots: HashMap<PathBuf, Vec<SimpleGlob>>,
}

impl WalkRoots {
    /// Build the `WalkRoots` by iterating through the globs and extracting and normaliziong
    /// any non-glob component so the input glob is split between the prefix and the actual glob
    /// the prefix containing all the semantic literals, meaning these need to be interpreted as
    /// modification to the search path
    pub fn build<'t>(globs: impl IntoIterator<Item = &'t str>) -> Result<Self, WalkRootsError> {
        let mut roots = HashMap::new();

        for glob in globs {
            let negated = glob.starts_with('!');

            // Remove ! from globs for processing
            let glob = if negated { &glob[1..] } else { glob };

            let (prefix, glob) = split_path_and_glob(glob);
            if glob.is_empty() {
                return Err(WalkRootsError::EmptyGlob {
                    prefix: prefix.to_string(),
                    glob: glob.to_string(),
                });
            }
            let mut normalized_prefix = normalize_relative(Path::new(prefix));

            let glob = if negated && !normalized_prefix.as_os_str().is_empty() && glob == "**" {
                normalized_prefix = PathBuf::new();
                format!("{}{}", prefix, glob)
            } else {
                glob.to_string()
            };

            roots
                .entry(normalized_prefix)
                .or_insert_with(Vec::new)
                .push(SimpleGlob::new(glob, negated));
        }

        Ok(Self { roots })
    }

    pub fn iter(&self) -> WalkRootsIter<'_> {
        WalkRootsIter {
            inner: self.roots.iter(),
        }
    }

    pub fn get(&self, root: &Path) -> Option<WalkRootItem<'_>> {
        self.roots
            .get_key_value(root)
            .map(|(path, globs)| WalkRootItem {
                path: path.as_path(),
                globs: globs.as_slice(),
            })
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

// Iteration interfaces

#[derive(Clone, Copy)]
pub struct WalkRootItem<'a> {
    path: &'a Path,
    globs: &'a [SimpleGlob],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimpleGlobItem<'a> {
    pub pattern: &'a str,
    pub negated: bool,
}

impl SimpleGlobItem<'_> {
    pub fn to_pattern(self) -> String {
        if self.negated {
            format!("!{}", self.pattern)
        } else {
            self.pattern.to_string()
        }
    }
}

// Iterator interfaces

/// Iterator over WalkRoots
pub struct WalkRootsIter<'a> {
    inner: std::collections::hash_map::Iter<'a, PathBuf, Vec<SimpleGlob>>,
}

/// Iterator over collections of SimpleGlob
pub struct SimpleGlobsIter<'a> {
    inner: std::slice::Iter<'a, SimpleGlob>,
}

impl<'a> Iterator for WalkRootsIter<'a> {
    type Item = WalkRootItem<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(path, globs)| WalkRootItem {
            path: path.as_path(),
            globs: globs.as_slice(),
        })
    }
}

// Iterator implementations

impl ExactSizeIterator for WalkRootsIter<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<'a> IntoIterator for &'a WalkRoots {
    type Item = WalkRootItem<'a>;
    type IntoIter = WalkRootsIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> WalkRootItem<'a> {
    pub fn path(&self) -> &'a Path {
        self.path
    }

    pub fn globs(&self) -> SimpleGlobsIter<'a> {
        SimpleGlobsIter {
            inner: self.globs.iter(),
        }
    }
}

impl<'a> IntoIterator for WalkRootItem<'a> {
    type Item = SimpleGlobItem<'a>;
    type IntoIter = SimpleGlobsIter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        SimpleGlobsIter {
            inner: self.globs.iter(),
        }
    }
}

impl<'a> Iterator for SimpleGlobsIter<'a> {
    type Item = SimpleGlobItem<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|glob| SimpleGlobItem {
            pattern: glob.pattern(),
            negated: glob.is_negated(),
        })
    }
}

impl ExactSizeIterator for SimpleGlobsIter<'_> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// Split a pattern into (path_prefix, glob_part).
/// - `path_prefix` ends at the last separator before the first glob metachar (`* ? [ {`)
///   and includes that separator (e.g. "src/").
/// - `glob_part` is the rest starting from the component that contains the first meta.
///
/// If no glob is present, returns ("", input).
pub fn split_path_and_glob(input: &str) -> (&str, &str) {
    fn is_meta(c: char) -> bool {
        matches!(c, '*' | '?' | '[' | '{')
    }

    fn is_sep(c: char) -> bool {
        c == '/'
    }
    for (i, ch) in input.char_indices() {
        if is_meta(ch) {
            // Find the last separator *before* the first meta.
            if let Some(sep_idx) = input[..i].rfind(|c: char| is_sep(c)) {
                // Include the separator in the path prefix.
                return (&input[..=sep_idx], &input[sep_idx + 1..]);
            } else {
                // Glob starts in the first path component.
                return ("", input);
            }
        }
    }

    // No glob characters found.
    ("", input)
}

/// Normalize paths like `../.././` into paths like `../../`
pub fn normalize_relative(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::CurDir => {}
            _ => out.push(comp.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::glob_set::walk_roots::normalize_relative;

    use super::{WalkRoots, split_path_and_glob};
    use insta::assert_yaml_snapshot;
    use serde::Serialize;

    #[derive(Serialize)]
    struct SnapshotRoot {
        path: String,
        globs: Vec<SnapshotGlob>,
    }

    #[derive(Serialize)]
    struct SnapshotGlob {
        pattern: String,
        negated: bool,
    }

    fn snapshot_walk_roots(walk_roots: &WalkRoots) -> Vec<SnapshotRoot> {
        let mut roots: Vec<_> = walk_roots
            .iter()
            .map(|root| SnapshotRoot {
                path: root.path().display().to_string(),
                globs: root
                    .globs()
                    .map(|g| SnapshotGlob {
                        pattern: g.pattern.to_string(),
                        negated: g.negated,
                    })
                    .collect(),
            })
            .collect();
        roots.sort_by(|a, b| a.path.cmp(&b.path));
        roots
    }

    #[test]
    fn test_split_path_and_glob() {
        assert_eq!(
            split_path_and_glob("../.././../*.{rs,cc}"),
            ("../.././../", "*.{rs,cc}")
        );
        assert_eq!(
            split_path_and_glob("src/*/test?.rs"),
            ("src/", "*/test?.rs")
        );
        assert_eq!(split_path_and_glob("*.rs"), ("", "*.rs"));
        assert_eq!(split_path_and_glob("plain/path"), ("", "plain/path"));
        assert_eq!(split_path_and_glob("foo[ab]/bar"), ("", "foo[ab]/bar"));
        assert_eq!(split_path_and_glob("pixi.toml"), ("", "pixi.toml"));
    }

    #[test]
    fn test_normalize() {
        assert_eq!(
            normalize_relative(Path::new("./.././.././")),
            Path::new("../../")
        );
    }

    #[test]
    fn determine_groups_globs_by_normalized_prefix() {
        let globs = [
            "./src/**/*.rs",
            "!./src/**/*.tmp",
            "../include/*.c",
            "!.pixi/**",
            "**/*.cpp",
        ];

        let walk_roots = WalkRoots::build(globs).expect("determine should succeed");

        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots),
            { ".**" => insta::sorted_redaction() },
            @r###"
        - globs:
            - pattern: ".pixi/**"
              negated: true
            - pattern: "**/*.cpp"
              negated: false
          path: ""
        - globs:
            - pattern: "*.c"
              negated: false
          path: "../include"
        - globs:
            - pattern: "**/*.rs"
              negated: false
            - pattern: "**/*.tmp"
              negated: true
          path: src
        "###
        );
    }

    #[test]
    fn determine_handles_globs_without_prefix() {
        let globs = ["*.rs", "!*.tmp"];

        let walk_roots = WalkRoots::build(globs).expect("determine should succeed");

        assert_eq!(walk_roots.len(), 1);
        assert!(!walk_roots.is_empty());
        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots),
            { ".**" => insta::sorted_redaction() },
            @r###"
        - globs:
            - pattern: "*.rs"
              negated: false
            - pattern: "*.tmp"
              negated: true
          path: ""
        "###
        );
    }

    #[test]
    fn iterates_over_roots_and_globs() {
        let globs = ["src/**/*.rs", "!src/**/generated.rs", "docs/**/*.md"];

        let walk_roots = WalkRoots::build(globs).expect("determine should succeed");
        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots),
            { ".**" => insta::sorted_redaction() },
            @r###"
        - globs:
            - pattern: "**/*.md"
              negated: false
          path: docs
        - globs:
            - pattern: "**/*.rs"
              negated: false
            - pattern: "**/generated.rs"
              negated: true
          path: src
        "###
        );
    }

    #[test]
    fn determine_negated_directory_glob_sticks_to_root() {
        let globs = ["!.pixi/**"];

        let walk_roots = WalkRoots::build(globs).expect("determine should succeed");

        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots),
            { ".**" => insta::sorted_redaction() },
            @r###"
        - globs:
            - pattern: ".pixi/**"
              negated: true
          path: ""
        "###
        );
    }
}
