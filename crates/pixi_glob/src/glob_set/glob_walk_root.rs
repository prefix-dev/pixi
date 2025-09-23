//! Plan the effective glob walk root for a set of patterns that may contain relative components.
//!
//! The builder determines how many `..` segments we need to traverse so every pattern can be
//! evaluated from a single ancestor directory.  When `rebase` is invoked we pop that ancestor off
//! the provided search root, splice the remaining literal components back into each pattern and
//! return the rewritten globs.  Negated patterns that start with `**/` are treated as global
//! exclusions and are emitted unchanged so users can keep wildcard directory bans in scope even if
//! the effective root moves.

use std::path::{Component, Path, PathBuf};

/// Simple handler to work with our globs
/// basically splits up negation
#[derive(Clone, Debug)]
pub struct SimpleGlob {
    glob: String,
    negated: bool,
}

impl SimpleGlob {
    pub fn new(glob: String, negated: bool) -> Self {
        Self { glob, negated }
    }

    /// Returns the pattern without leading !
    pub fn normalized_pattern(&self) -> &str {
        &self.glob
    }

    pub fn is_negated(&self) -> bool {
        self.negated
    }

    /// Returns a proper glob pattern
    pub fn to_pattern(&self) -> String {
        if self.negated {
            format!("!{}", self.glob)
        } else {
            self.glob.clone()
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WalkRootsError {
    #[error("after processing glob '{glob}', split into '{prefix}' and empty glob")]
    EmptyGlob { prefix: String, glob: String },

    #[error("glob prefix '{prefix}' must be relative")]
    AbsolutePrefix { prefix: String },

    #[error("cannot ascend {required} level(s) from '{root}'")]
    CannotAscend { required: usize, root: PathBuf },
}

struct GlobSpec {
    // Is this a ! glob
    negated: bool,
    // How many `..` path components does this contain
    parent_dirs: usize,
    // The `foo/bar/` concrete components
    concrete_components: Vec<String>,
    // Original glob pattern
    pattern: String,
    // Determines if we want to rebase the glob
    skip_rebase: bool,
}

/// Contains the globs and the joinable path
pub struct GlobWalkRoot {
    // The parsed glob specifications
    specs: Vec<GlobSpec>,
    // The maximum number of parent dirs we need to ascend
    max_parent_dirs: usize,
}

/// Globs rebased to a common root
pub struct RebasedGlobs {
    // The new root directory to search from
    pub root: PathBuf,
    // The globs with the rebased patterns
    pub globs: Vec<SimpleGlob>,
}

impl GlobWalkRoot {
    /// Build a list of globs into a structure that we can use to rebase or reparent
    /// the globs when given
    pub fn build<'t>(globs: impl IntoIterator<Item = &'t str>) -> Result<Self, WalkRootsError> {
        let mut specs = Vec::new();
        let mut max_parent_dirs = 0usize;

        for glob in globs {
            let negated = glob.starts_with('!');
            let glob = if negated { &glob[1..] } else { glob };

            // First split of any relative part information
            let (prefix, pattern) = split_path_and_glob(glob);

            // Having an empty glob is an error
            if pattern.is_empty() {
                return Err(WalkRootsError::EmptyGlob {
                    prefix: prefix.to_string(),
                    glob: glob.to_string(),
                });
            }

            let normalized_prefix = normalize_relative(Path::new(prefix));
            // This will determine how we need to rebase the globs
            let mut parent_dirs = 0usize;
            let mut concrete_components = Vec::new();

            // Loop over components and split into concrete and relative parts
            for comp in normalized_prefix.components() {
                match comp {
                    Component::ParentDir => parent_dirs += 1,
                    Component::CurDir => {}
                    Component::Normal(s) => {
                        concrete_components.push(s.to_string_lossy().into_owned());
                    }
                    Component::RootDir | Component::Prefix(_) => {
                        return Err(WalkRootsError::AbsolutePrefix {
                            prefix: prefix.to_string(),
                        });
                    }
                }
            }

            // We skip !**/ patterns for rebasing, as we would probably always want to apply those
            let skip_rebase =
                negated && normalized_prefix.as_os_str().is_empty() && pattern.starts_with("**/");

            max_parent_dirs = max_parent_dirs.max(parent_dirs);
            specs.push(GlobSpec {
                negated,
                parent_dirs,
                concrete_components,
                pattern: pattern.to_string(),
                skip_rebase,
            });
        }

        Ok(Self {
            specs,
            max_parent_dirs,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Rebase the globs into the designated roots
    /// How this rebasing works is determined by the input globs.
    /// This only actually does something when we have some "relative" globs
    /// Like `../../*.rs` or something of the sort
    pub fn rebase(&self, root: &Path) -> Result<RebasedGlobs, WalkRootsError> {
        if self.specs.is_empty() {
            return Ok(RebasedGlobs {
                root: root.to_path_buf(),
                globs: Vec::new(),
            });
        }

        // Count all available components in the path
        let available = root
            .components()
            .filter(|c| matches!(c, Component::Normal(_) | Component::Prefix(_)))
            .count();

        if available < self.max_parent_dirs {
            // This happens when we have a glob somewhere like
            // `../../../foo` but we try to search in `/tmp`
            // in that case we cannot ascend up high enough
            return Err(WalkRootsError::CannotAscend {
                required: self.max_parent_dirs,
                root: root.to_path_buf(),
            });
        }

        // We are going to modify till we get to the root
        let mut effective_root = root.to_path_buf();
        let mut popped = Vec::with_capacity(self.max_parent_dirs);
        for _ in 0..self.max_parent_dirs {
            let name = effective_root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .expect("bug: checked available components beforehand");
            effective_root.pop();
            popped.push(name);
        }
        popped.reverse();

        let mut rebased = Vec::with_capacity(self.specs.len());
        for spec in &self.specs {
            // Skip rebasing entirely
            if spec.skip_rebase {
                rebased.push(SimpleGlob::new(spec.pattern.clone(), spec.negated));
                continue;
            }

            let keep_from_prefix = self.max_parent_dirs.saturating_sub(spec.parent_dirs);

            // Create the glob prefix
            let mut components = Vec::new();
            components.extend(popped.iter().take(keep_from_prefix).cloned());
            components.extend(spec.concrete_components.iter().cloned());

            let rebased_pattern = if components.is_empty() {
                // No rebasing needs to be performed
                spec.pattern.clone()
            } else {
                // Rebase the glob with the calculated parent
                format!("{}/{}", components.join("/"), spec.pattern)
            };

            rebased.push(SimpleGlob::new(rebased_pattern, spec.negated));
        }

        Ok(RebasedGlobs {
            root: effective_root,
            globs: rebased,
        })
    }
}

/// Split a pattern into (path_prefix, glob_part).
/// - `path_prefix` ends at the last separator before the first glob metachar (`* ? [ {`)
///   and includes that separator (e.g. "src/").
/// - `glob_part` is the rest starting from the component that contains the first meta.
///   If no glob is present, returns ("", input).
///
/// Examples:
///   "../.././../*.{rs,cc}" -> ("../.././../", "*.{rs,cc}")
///   "src/*/test?.rs"      -> ("src/", "*/test?.rs")
///   "*.rs"                -> ("", "*.rs")
///   "plain/path"          -> ("", "plain/path")
pub fn split_path_and_glob(input: &str) -> (&str, &str) {
    fn is_meta(c: char) -> bool {
        matches!(c, '*' | '?' | '[' | '{')
    }

    fn is_sep(c: char) -> bool {
        c == '/'
    }
    for (i, ch) in input.char_indices() {
        if is_meta(ch) {
            if let Some(sep_idx) = input[..i].rfind(|c: char| is_sep(c)) {
                return (&input[..=sep_idx], &input[sep_idx + 1..]);
            } else {
                return ("", input);
            }
        }
    }

    // In this case we have not found any meta patterns and we can assume the glob can be in the form of a file match like
    // foo/bar.txt, because we will need to add a current directory `./` separator as we are using ignore and gitignore style
    // glob rules
    ("", input)
}

/// Normalize paths like `../.././` into paths like `../../`
pub fn normalize_relative(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            _ => out.push(comp.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{GlobWalkRoot, normalize_relative, split_path_and_glob};
    use insta::assert_yaml_snapshot;
    use serde::Serialize;

    #[derive(Serialize)]
    struct SnapshotWalk {
        root: String,
        globs: Vec<SnapshotGlob>,
    }

    #[derive(Serialize)]
    struct SnapshotGlob {
        pattern: String,
        negated: bool,
    }

    fn snapshot_walk_roots(plan: &GlobWalkRoot, root: &Path) -> SnapshotWalk {
        let rebased = plan.rebase(root).expect("rebase should succeed");
        let root_str = rebased.root.display().to_string().replace('\\', "/");
        let globs = rebased
            .globs
            .iter()
            .map(|g| SnapshotGlob {
                pattern: g.normalized_pattern().to_string(),
                negated: g.is_negated(),
            })
            .collect();
        SnapshotWalk {
            root: root_str,
            globs,
        }
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

    // Couple of test cases to check that rebasing works as expected
    #[test]
    fn determine_groups_globs_by_normalized_prefix() {
        let globs = [
            "./src/**/*.rs",
            "!./src/**/*.tmp",
            "../include/*.c",
            "!.pixi/**",
            "!**/.pixi/**",
            "**/*.cpp",
        ];

        let walk_roots = GlobWalkRoot::build(globs).expect("determine should succeed");

        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots, Path::new("workspace/baz")),
            @r###"
        root: workspace
        globs:
          - pattern: baz/src/**/*.rs
            negated: false
          - pattern: baz/src/**/*.tmp
            negated: true
          - pattern: include/*.c
            negated: false
          - pattern: baz/.pixi/**
            negated: true
          - pattern: "**/.pixi/**"
            negated: true
          - pattern: baz/**/*.cpp
            negated: false
        "###
        );
    }

    // Check that nothing happens when rebasing
    #[test]
    fn determine_handles_globs_without_prefix() {
        let globs = ["*.rs", "!*.tmp"];

        let walk_roots = GlobWalkRoot::build(globs).expect("determine should succeed");

        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots, Path::new("workspace/baz")),
            @r###"
        root: workspace/baz
        globs:
          - pattern: "*.rs"
            negated: false
          - pattern: "*.tmp"
            negated: true
        "###
        );
    }

    #[test]
    fn iterates_over_roots_and_globs() {
        let globs = ["src/**/*.rs", "!src/**/generated.rs", "docs/**/*.md"];

        let walk_roots = GlobWalkRoot::build(globs).expect("determine should succeed");
        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots, Path::new("workspace")),
            @r###"
        root: workspace
        globs:
          - pattern: src/**/*.rs
            negated: false
          - pattern: src/**/generated.rs
            negated: true
          - pattern: docs/**/*.md
            negated: false
        "###
        );
    }

    #[test]
    fn determine_negated_directory_glob_sticks_to_root() {
        let globs = ["!.pixi/**", "../*.{cc,cpp}"];

        let walk_roots = GlobWalkRoot::build(globs).expect("determine should succeed");

        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots, Path::new("workspace/baz")),
            @r###"
        root: workspace
        globs:
          - pattern: baz/.pixi/**
            negated: true
          - pattern: "*.{cc,cpp}"
            negated: false
        "###
        );
    }

    #[test]
    fn single_file_match() {
        let globs = ["pixi.toml", "../*.{cc,cpp}"];

        let walk_roots = GlobWalkRoot::build(globs).expect("determine should succeed");

        assert_yaml_snapshot!(
            snapshot_walk_roots(&walk_roots, Path::new("workspace/baz")),
            @r###"
        root: workspace
        globs:
          - pattern: baz/pixi.toml
            negated: false
          - pattern: "*.{cc,cpp}"
            negated: false
        "###
        );
    }
}
