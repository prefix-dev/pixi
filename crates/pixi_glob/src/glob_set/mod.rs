//! Convenience wrapper around `ignore` that emulates the glob semantics pixi expects.
//!
//! Notable behavioural tweaks compared to vanilla gitignore parsing, so that it behaves more like unix globbing with special rules:
//! - Globs are rebased to a shared search root so patterns like `../src/*.rs` keep working even
//!   when the caller starts from a nested directory.
//! - Negated patterns that start with `**/` are treated as global exclusions. We skip rebasing
//!   those so `!**/build.rs` still hides every `build.rs`, regardless of the effective root.
//! - Plain file names without meta characters (e.g. `pixi.toml`) are anchored to the search root
//!   instead of matching anywhere below it. This mirrors the behaviour we had with the previous
//!   wax-based implementation.
//! - Negated literals (e.g. `!pixi.toml`) are anchored the same way, which lets recipes ignore a
//!   single file at the root without accidentally hiding copies deeper in the tree.

mod walk;
mod walk_root;

use std::{
    fs::Metadata,
    io,
    path::{Path, PathBuf},
};

use thiserror::Error;

use walk_root::{WalkRoot, WalkRootsError};

/// A single result from [`GlobSet::collect_matching`].
///
/// `Pattern` matches retain the underlying `ignore::DirEntry` so callers
/// can reuse the metadata cached by the directory walk on platforms that
/// hand it back from `readdir` (notably Windows).  `Leaf` matches come
/// from the leaf-marker side channel: the walker only ever saw the
/// containing directory, so callers that need metadata have to stat the
/// file themselves.
#[derive(Debug)]
pub enum Match {
    /// Result of a positive glob match emitted by the walker.
    Pattern(ignore::DirEntry),
    /// Leaf marker hit; the walker pruned the subtree after recording it.
    Leaf(PathBuf),
}

impl Match {
    /// Borrowed view of the matched path.
    pub fn path(&self) -> &Path {
        match self {
            Match::Pattern(d) => d.path(),
            Match::Leaf(p) => p.as_path(),
        }
    }

    /// Consume the match, returning the owned path.
    pub fn into_path(self) -> PathBuf {
        match self {
            Match::Pattern(d) => d.into_path(),
            Match::Leaf(p) => p,
        }
    }

    /// Resolve the entry's metadata, reusing the walker's cached copy for
    /// `Pattern` matches and falling back to a fresh `stat` for `Leaf`
    /// matches.
    pub fn metadata(&self) -> io::Result<Metadata> {
        match self {
            Match::Pattern(d) => d.metadata().map_err(io::Error::other),
            Match::Leaf(p) => fs_err::metadata(p),
        }
    }
}

/// A glob set implemented using the `ignore` crate (globset + fast walker).
///
/// In addition to gitignore-style patterns, callers can declare *markers*:
/// file names that the walker looks for in every directory it enters.
/// When a marker file is present, its full path is matched against the
/// pattern set:
///
/// - matches an include pattern → the marker is recorded as a result and
///   descent under that directory stops (leaf semantics);
/// - matches anything else (an exclude `!` pattern, or no pattern at all)
///   → the entire subtree is skipped (prune semantics).
///
/// In other words, listed markers always affect the walk; include patterns
/// promote them from "prune" (default) to "leaf".  This lets a single
/// pattern set express both ordinary glob matching and the structural
/// pruning / leaf detection needed by workspace-discovery callers (e.g.
/// ROS).
pub struct GlobSet {
    /// Include/exclude patterns (gitignore-style), grouped by their walk root.
    pub walk_roots: WalkRoot,
    /// File names whose presence in a directory triggers a per-dir
    /// override-match against [`GlobSet::walk_roots`].  See the type docs.
    pub markers: Vec<String>,
    /// When true (the default), hidden files and directories (names that
    /// start with `.`) are skipped during the walk unless the user's
    /// patterns explicitly opt them in.
    pub exclude_hidden: bool,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum GlobSetError {
    #[error("failed to build globs")]
    BuildOverrides(#[source] ignore::Error),

    #[error("walk error at {0}")]
    Walk(PathBuf, #[source] ignore::Error),

    #[error(transparent)]
    WalkRoots(#[from] WalkRootsError),
}

impl GlobSet {
    /// Create a new [`GlobSet`] from a list of patterns. Leading '!' indicates exclusion.
    pub fn create<'t>(globs: impl IntoIterator<Item = &'t str>) -> GlobSet {
        GlobSet {
            walk_roots: WalkRoot::build(globs).expect("should not fail"),
            markers: Vec::new(),
            exclude_hidden: true,
        }
    }

    /// Builder-style setter for [`GlobSet::exclude_hidden`].  Pass `false`
    /// to walk into hidden directories regardless of the pattern shape.
    pub fn with_exclude_hidden(mut self, exclude_hidden: bool) -> Self {
        self.exclude_hidden = exclude_hidden;
        self
    }

    /// Builder-style setter that replaces [`GlobSet::markers`].  Each
    /// supplied file name causes the walker, on entering a directory, to
    /// probe for that file and apply the leaf-or-prune dispatch described
    /// in the type-level documentation.
    pub fn with_ignore_marker_filenames<'m, I>(mut self, filenames: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str> + 'm,
    {
        self.markers = filenames
            .into_iter()
            .map(|s| s.as_ref().to_owned())
            .collect();
        self
    }

    /// Walks `root_dir` collecting [`Match`] entries: files that match the
    /// configured patterns plus per-directory leaf markers, while pruning at
    /// directories whose marker hits an exclude pattern.  Ordering is not
    /// guaranteed.
    pub fn collect_matching(&self, root_dir: &Path) -> Result<Vec<Match>, GlobSetError> {
        let has_patterns = !self.walk_roots.is_empty();
        let has_markers = !self.markers.is_empty();
        if !has_patterns && !has_markers {
            return Ok(Vec::new());
        }

        let (root, globs) = if has_patterns {
            let rebased = self.walk_roots.rebase(root_dir)?;
            (rebased.root, rebased.globs)
        } else {
            (root_dir.to_path_buf(), Vec::new())
        };

        walk::walk_globs(&root, &globs, &self.markers, self.exclude_hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::{GlobSet, Match};
    use fs_err::{self as fs, File};
    use insta::assert_yaml_snapshot;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    fn relative_path(path: &Path, root: &Path) -> PathBuf {
        if let Ok(rel) = path.strip_prefix(root) {
            return rel.to_path_buf();
        }
        if let Some(parent) = root.parent()
            && let Ok(rel) = path.strip_prefix(parent)
        {
            return std::path::Path::new("..").join(rel);
        }
        path.to_path_buf()
    }

    fn sorted_paths(entries: Vec<Match>, root: &std::path::Path) -> Vec<String> {
        let mut paths: Vec<_> = entries
            .into_iter()
            .map(|m| {
                relative_path(&m.into_path(), root)
                    .display()
                    .to_string()
                    .replace('\\', "/")
            })
            .collect();
        paths.sort();
        paths
    }

    // Test out a normal non-reseated globbing approach
    #[test]
    fn collect_matching_inclusion_exclusion() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        File::create(root_path.join("include1.txt")).unwrap();
        File::create(root_path.join("include2.log")).unwrap();
        File::create(root_path.join("exclude.txt")).unwrap();
        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/include_subdir.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["**/*.txt", "!exclude.txt"]);
        let entries = glob_set.collect_matching(root_path).unwrap();

        let paths = sorted_paths(entries, root_path);
        assert_yaml_snapshot!(paths, @r###"
        - include1.txt
        - subdir/include_subdir.txt
        "###);
    }

    // Pin the gitignore "last-match-wins" semantics that `GlobSet` inherits
    // from `ignore::overrides::Override`. Callers that emit a mix of
    // inclusion and exclusion patterns must keep exclusions *after* the
    // inclusions they intend to negate, otherwise the negation is undone by
    // the later, broader include.
    #[test]
    fn collect_matching_order_is_last_match_wins() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();
        File::create(root_path.join("include1.txt")).unwrap();
        File::create(root_path.join("exclude.txt")).unwrap();

        // Exclude before include: the include re-matches `exclude.txt`.
        let exclude_then_include = GlobSet::create(vec!["!exclude.txt", "**/*.txt"]);
        let paths = sorted_paths(
            exclude_then_include.collect_matching(root_path).unwrap(),
            root_path,
        );
        assert_eq!(paths, vec!["exclude.txt", "include1.txt"]);

        // Include before exclude: exclusion sticks.
        let include_then_exclude = GlobSet::create(vec!["**/*.txt", "!exclude.txt"]);
        let paths = sorted_paths(
            include_then_exclude.collect_matching(root_path).unwrap(),
            root_path,
        );
        assert_eq!(paths, vec!["include1.txt"]);
    }

    // Check some general globbing support and make sure the correct things do not match
    #[test]
    fn collect_matching_relative_globs() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();
        let search_root = root_path.join("workspace");
        fs::create_dir(&search_root).unwrap();

        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/some_inner_source.cpp")).unwrap();
        File::create(root_path.join("subdir/dont-match.txt")).unwrap();
        File::create(search_root.join("match.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["../**/*.cpp", "*.txt"]);
        let entries = glob_set.collect_matching(&search_root).unwrap();

        let paths = sorted_paths(entries, &search_root);
        assert_yaml_snapshot!(paths, @r###"
        - "../subdir/some_inner_source.cpp"
        - match.txt
        "###);
    }

    // Check that single matching file glob works with rebasing
    #[test]
    fn collect_matching_file_glob() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml", "../*.cpp"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- pixi.toml");
    }

    // Check that global ignores !**/ patterns ignore everything even if the root has been
    // rebased to a parent folder, this is just a convenience assumed to be preferable
    // from a user standpoint
    #[test]
    fn check_global_ignore_ignores() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();
        File::create(root_path.join("foo.txt")).unwrap();
        // This would be picked up otherwise
        File::create(temp_dir.path().join("foo.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml", "!**/foo.txt"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- pixi.toml");
    }

    // Check that we can ignore a subset of file when using the rebasing
    // So we want to match all `.txt` and `*.toml` files except in the root location
    // where want to exclude `foo.txt`
    #[test]
    fn check_subset_ignore() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("pixi.toml")).unwrap();
        // This should not be picked up
        File::create(root_path.join("foo.txt")).unwrap();
        // But because of the non-global ignore this should be
        File::create(temp_dir.path().join("foo.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["../*.{toml,txt}", "!foo.txt"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r###"
        - "../foo.txt"
        - pixi.toml
        "###);
    }

    #[test]
    fn check_we_ignore_hidden_files() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi");

        fs::create_dir(&hidden_pixi_folder).unwrap();
        // This should not be picked up
        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();
        // But because of the non-global ignore this should be
        File::create(root_path.as_path().join("foo_public.txt")).unwrap();

        let glob_set = GlobSet::create(vec!["*.txt"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @"- foo_public.txt");
    }

    #[test]
    fn check_hidden_folders_are_included() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi");

        let hidden_foobar_folder = root_path.join(".foobar");

        let hidden_recursive_folder = root_path
            .join("recursive")
            .join("foobar")
            .join(".deep_hidden");

        fs::create_dir(&hidden_pixi_folder).unwrap();
        fs::create_dir(&hidden_foobar_folder).unwrap();
        fs::create_dir_all(&hidden_recursive_folder).unwrap();

        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();
        File::create(hidden_foobar_folder.as_path().join("foo_from_foobar.txt")).unwrap();
        File::create(hidden_foobar_folder.as_path().join("build.txt")).unwrap();

        File::create(hidden_recursive_folder.join("foo_from_deep_hidden.txt")).unwrap();

        File::create(root_path.as_path().join("some_text.txt")).unwrap();
        let glob_set = GlobSet::create(vec![
            "**",
            ".foobar/foo_from_foobar.txt",
            "**/.deep_hidden/**",
        ]);

        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r#"
        - ".foobar/foo_from_foobar.txt"
        - recursive/foobar/.deep_hidden/foo_from_deep_hidden.txt
        - some_text.txt
        "#);
    }

    #[test]
    fn check_hidden_folder_is_whitelisted_with_star() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi").join("subdir");

        fs::create_dir_all(&hidden_pixi_folder).unwrap();

        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();

        File::create(root_path.as_path().join("some_text.txt")).unwrap();
        let glob_set = GlobSet::create(vec![".pixi/subdir/**"]);

        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r#"
        - ".pixi/subdir/foo_hidden.txt"
        "#);
    }

    #[test]
    fn check_hidden_folders_are_not_included() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        let hidden_pixi_folder = root_path.join(".pixi");

        fs::create_dir(&hidden_pixi_folder).unwrap();

        File::create(hidden_pixi_folder.join("foo_hidden.txt")).unwrap();

        File::create(root_path.as_path().join("some_text.txt")).unwrap();
        // We want to match everything except hidden folders
        let glob_set = GlobSet::create(vec!["**"]);

        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r#"
        - some_text.txt
        "#);
    }

    /// Because we are using ignore which uses gitignore style parsing of globs we need to do some extra processing
    /// to make this more like unix globs in this case we check this explicitly here
    #[test]
    fn single_file_match() {
        let temp_dir = tempdir().unwrap();
        let workspace = temp_dir.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let subdir = workspace.join("subdir");
        fs::create_dir(&subdir).unwrap();

        File::create(subdir.join("pixi.toml")).unwrap();

        let glob_set = GlobSet::create(vec!["pixi.toml"]);
        let entries = glob_set.collect_matching(&workspace).unwrap();

        let paths = sorted_paths(entries, &workspace);
        assert_yaml_snapshot!(paths, @"[]");
    }

    /// Symlinks to directories should be followed, so files inside the target
    /// directory are discovered. Reproduces https://github.com/prefix-dev/pixi/issues/5417
    #[cfg(unix)]
    #[test]
    fn symlink_to_directory_is_followed() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        // Create a real directory with files *outside* the search root
        let real_dir = temp_dir.path().join("real_dir");
        fs::create_dir(&real_dir).unwrap();
        File::create(real_dir.join("linked_file.txt")).unwrap();

        // Create a regular file and directory in the search root
        File::create(root_path.join("regular.txt")).unwrap();

        // Create a symlink inside the search root pointing to the real directory
        std::os::unix::fs::symlink(&real_dir, root_path.join("link_dir")).unwrap();

        let glob_set = GlobSet::create(vec!["**"]);
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r#"
        - link_dir/linked_file.txt
        - regular.txt
        "#);
    }

    /// A symlink that forms a cycle (a link whose target is one of its own
    /// ancestors) must not abort the walk. This is the shape pnpm/npm create in
    /// workspace `node_modules`, where packages link back to the workspace root.
    /// The looping link is skipped and every other file is still collected,
    /// rather than the walk failing with `GlobSetError::Walk`.
    #[cfg(unix)]
    #[test]
    fn symlink_loop_is_skipped_not_fatal() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path().join("workspace");
        fs::create_dir(&root_path).unwrap();

        File::create(root_path.join("regular.txt")).unwrap();

        // `nested/` holds a real file plus a symlink pointing back to its own
        // parent (`nested/loop -> ..`), i.e. to an ancestor. With
        // `follow_links(true)` the walker would otherwise recurse forever; it
        // instead reports a loop error for this entry.
        let nested = root_path.join("nested");
        fs::create_dir(&nested).unwrap();
        File::create(nested.join("inner.txt")).unwrap();
        std::os::unix::fs::symlink("..", nested.join("loop")).unwrap();

        let glob_set = GlobSet::create(vec!["**"]);
        // Before the fix this returned `Err(GlobSetError::Walk(..))`.
        let entries = glob_set.collect_matching(&root_path).unwrap();

        let paths = sorted_paths(entries, &root_path);
        assert_yaml_snapshot!(paths, @r#"
        - nested/inner.txt
        - regular.txt
        "#);
    }

    fn workspace_root_for_marker_tests() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root from pixi_glob manifest")
            .join("tests")
            .join("data")
            .join("pixi-build")
            .join("ros-workspace")
    }

    #[test]
    fn leaf_marker_finds_all_package_xml_in_ros_workspace() {
        let root = workspace_root_for_marker_tests();
        let glob_set =
            GlobSet::create(["**/package.xml"]).with_ignore_marker_filenames(["package.xml"]);
        let paths = sorted_paths(glob_set.collect_matching(&root).unwrap(), &root);
        assert_eq!(
            paths,
            vec![
                "src/distro_less_package/package.xml".to_string(),
                "src/navigator/package.xml".to_string(),
                "src/navigator_implicit/package.xml".to_string(),
                "src/navigator_py/package.xml".to_string(),
            ]
        );
    }

    #[test]
    fn leaf_marker_at_root_is_returned_as_single_hit() {
        let tmp = tempdir().unwrap();
        File::create(tmp.path().join("package.xml")).unwrap();
        fs::create_dir_all(tmp.path().join("nested/subdir")).unwrap();
        File::create(tmp.path().join("nested/subdir/package.xml")).unwrap();

        let glob_set =
            GlobSet::create(["**/package.xml"]).with_ignore_marker_filenames(["package.xml"]);
        let entries = glob_set.collect_matching(tmp.path()).unwrap();
        // Root has the leaf; descent stops there and the nested copy is invisible.
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path(), tmp.path().join("package.xml"));
    }

    #[test]
    fn prune_marker_hides_subtree() {
        let tmp = tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("a")).unwrap();
        File::create(tmp.path().join("a/package.xml")).unwrap();

        fs::create_dir_all(tmp.path().join("b")).unwrap();
        File::create(tmp.path().join("b/package.xml")).unwrap();
        File::create(tmp.path().join("b/COLCON_IGNORE")).unwrap();

        let glob_set = GlobSet::create(["**/package.xml"])
            .with_ignore_marker_filenames(["package.xml", "COLCON_IGNORE"]);
        let paths = sorted_paths(glob_set.collect_matching(tmp.path()).unwrap(), tmp.path());
        assert_eq!(paths, vec!["a/package.xml".to_string()]);
    }

    #[test]
    fn exclude_hidden_false_yields_hidden_files() {
        let tmp = tempdir().unwrap();
        let hidden_dir = tmp.path().join(".hidden");
        fs::create_dir_all(&hidden_dir).unwrap();
        File::create(hidden_dir.join("inside.txt")).unwrap();
        File::create(tmp.path().join("visible.txt")).unwrap();

        // Default (true) hides `.hidden/inside.txt`.
        let default_paths = sorted_paths(
            GlobSet::create(["**/*.txt"])
                .collect_matching(tmp.path())
                .unwrap(),
            tmp.path(),
        );
        assert_eq!(default_paths, vec!["visible.txt".to_string()]);

        // Opt out via `with_exclude_hidden(false)`.
        let inclusive_paths = sorted_paths(
            GlobSet::create(["**/*.txt"])
                .with_exclude_hidden(false)
                .collect_matching(tmp.path())
                .unwrap(),
            tmp.path(),
        );
        assert_eq!(
            inclusive_paths,
            vec![".hidden/inside.txt".to_string(), "visible.txt".to_string()]
        );
    }

    #[test]
    fn missing_root_returns_empty() {
        let tmp = tempdir().unwrap();
        let missing = tmp.path().join("does-not-exist");
        let glob_set =
            GlobSet::create(["**/package.xml"]).with_ignore_marker_filenames(["package.xml"]);
        let entries = glob_set.collect_matching(&missing).unwrap();
        assert!(entries.is_empty());
    }

    /// Build a synthetic ROS-style workspace and compare:
    ///   (a) today's path: `GlobSet::collect_matching` with the patterns
    ///       pixi-build-ros currently emits, run once per package.
    ///   (b) unified GlobSet with leaf + prune markers, run once for the
    ///       whole workspace.
    ///
    /// Run with:
    ///   cargo test -p pixi_glob --release -- --ignored bench_workspace_discovery --nocapture
    #[test]
    #[ignore = "manual benchmark; not deterministic enough for CI"]
    fn bench_workspace_discovery() {
        use std::collections::BTreeSet;
        use std::time::Instant;

        let tmp = tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        fs::create_dir_all(&workspace).unwrap();

        let pkg_count = 200usize;
        let files_per_pkg = 50usize;
        for i in 0..pkg_count {
            let pkg = workspace.join("src").join(format!("pkg_{i:03}"));
            fs::create_dir_all(pkg.join("src")).unwrap();
            fs::create_dir_all(pkg.join("include")).unwrap();
            File::create(pkg.join("package.xml")).unwrap();
            File::create(pkg.join("CMakeLists.txt")).unwrap();
            for j in 0..files_per_pkg {
                File::create(pkg.join("src").join(format!("a_{j:03}.cpp"))).unwrap();
                File::create(pkg.join("include").join(format!("a_{j:03}.h"))).unwrap();
            }
        }

        eprintln!(
            "synthetic workspace: {pkg_count} pkgs, {} files",
            pkg_count * (files_per_pkg * 2 + 2)
        );

        let pkg_dirs: Vec<PathBuf> = (0..pkg_count)
            .map(|i| workspace.join("src").join(format!("pkg_{i:03}")))
            .collect();
        let ros_globs = [
            "../../**/package.xml",
            "../../**/COLCON_IGNORE",
            "../../**/AMENT_IGNORE",
            "../../**/CATKIN_IGNORE",
            "!../../**/.*/**",
            "package.xml",
            "CMakeLists.txt",
            "setup.py",
            "setup.cfg",
        ];
        let mut all_glob_hits: BTreeSet<PathBuf> = BTreeSet::new();
        let glob_start = Instant::now();
        for pkg in &pkg_dirs {
            let glob_set = GlobSet::create(ros_globs.iter().copied());
            for hit in glob_set.collect_matching(pkg).unwrap() {
                all_glob_hits.insert(hit.into_path());
            }
        }
        let glob_elapsed = glob_start.elapsed();
        eprintln!(
            "GlobSet x {pkg_count} packages: {:?}, unique hits = {}",
            glob_elapsed,
            all_glob_hits.len()
        );

        let walk_start = Instant::now();
        let leaves = GlobSet::create(["**/package.xml"])
            .with_ignore_marker_filenames([
                "package.xml",
                "COLCON_IGNORE",
                "AMENT_IGNORE",
                "CATKIN_IGNORE",
            ])
            .collect_matching(&workspace)
            .unwrap();
        let walk_elapsed = walk_start.elapsed();
        eprintln!(
            "marker walk x 1: {:?}, leaves = {}",
            walk_elapsed,
            leaves.len()
        );

        if walk_elapsed.as_nanos() > 0 {
            let ratio = glob_elapsed.as_nanos() as f64 / walk_elapsed.as_nanos() as f64;
            eprintln!("speedup: {ratio:.1}x");
        }

        assert_eq!(leaves.len(), pkg_count);
    }

    /// Same comparison against a real workspace. Set `PIXI_BENCH_WS` to the
    /// workspace root before running.
    ///
    /// Run with:
    ///   $env:PIXI_BENCH_WS = "F:/projects/issues/navigation2"; \
    ///     cargo test -p pixi_glob --release -- --ignored bench_workspace_real --nocapture
    #[test]
    #[ignore = "needs PIXI_BENCH_WS env var pointing at a real workspace"]
    fn bench_workspace_real() {
        use std::collections::BTreeSet;
        use std::time::Instant;

        let Some(ws) = std::env::var_os("PIXI_BENCH_WS") else {
            eprintln!("PIXI_BENCH_WS unset; skipping");
            return;
        };
        let workspace = PathBuf::from(ws);
        eprintln!("workspace: {}", workspace.display());

        let marker_patterns = ["**/package.xml"];
        let marker_names = [
            "package.xml",
            "COLCON_IGNORE",
            "AMENT_IGNORE",
            "CATKIN_IGNORE",
        ];
        let leaves: Vec<PathBuf> = GlobSet::create(marker_patterns)
            .with_ignore_marker_filenames(marker_names)
            .collect_matching(&workspace)
            .unwrap()
            .into_iter()
            .map(|m| m.into_path())
            .collect();
        let pkg_dirs: Vec<PathBuf> = leaves
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .collect();
        eprintln!("packages found: {}", pkg_dirs.len());

        let ros_globs = [
            "../../**/package.xml",
            "../../**/COLCON_IGNORE",
            "../../**/AMENT_IGNORE",
            "../../**/CATKIN_IGNORE",
            "!../../**/.*/**",
            "package.xml",
            "CMakeLists.txt",
            "setup.py",
            "setup.cfg",
        ];

        let mut all_glob_hits: BTreeSet<PathBuf> = BTreeSet::new();
        let glob_start = Instant::now();
        for pkg in &pkg_dirs {
            let glob_set = GlobSet::create(ros_globs.iter().copied());
            for hit in glob_set.collect_matching(pkg).unwrap() {
                all_glob_hits.insert(hit.into_path());
            }
        }
        let glob_elapsed = glob_start.elapsed();
        eprintln!(
            "GlobSet x {} packages: {:?}, unique hits = {}",
            pkg_dirs.len(),
            glob_elapsed,
            all_glob_hits.len()
        );

        let walk_start = Instant::now();
        let leaves2 = GlobSet::create(marker_patterns)
            .with_ignore_marker_filenames(marker_names)
            .collect_matching(&workspace)
            .unwrap();
        let walk_elapsed = walk_start.elapsed();
        eprintln!(
            "marker walk x 1: {:?}, leaves = {}",
            walk_elapsed,
            leaves2.len()
        );

        if walk_elapsed.as_nanos() > 0 {
            let ratio = glob_elapsed.as_nanos() as f64 / walk_elapsed.as_nanos() as f64;
            eprintln!("speedup: {ratio:.1}x");
        }
    }
}
