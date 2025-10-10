//! Contains the directory walking implementation
use itertools::Itertools;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::glob_set::walk_root::SimpleGlob;

use super::GlobSetError;

type SharedResults = Arc<Mutex<Option<Vec<Result<ignore::DirEntry, GlobSetError>>>>>;

struct CollectBuilder {
    // Shared aggregation storage wrapped in an Option so we can `take` at the end.
    sink: SharedResults,
    // The root we are walking, used for error reporting
    err_root: PathBuf,
}

struct CollectVisitor {
    // Local per-thread buffer to append results without holding the lock.
    local: Vec<Result<ignore::DirEntry, GlobSetError>>,
    // Reference to the shared sink.
    sink: SharedResults,
    // The root we are walking, used for error reporting
    err_root: PathBuf,
}

impl Drop for CollectVisitor {
    // This merges the outputs on the drop
    fn drop(&mut self) {
        let mut sink = self.sink.lock();
        sink.get_or_insert_with(Vec::new).append(&mut self.local);
    }
}

impl<'s> ignore::ParallelVisitorBuilder<'s> for CollectBuilder {
    fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
        // Build a visitor that maintains an internal list
        Box::new(CollectVisitor {
            local: Vec::new(),
            sink: Arc::clone(&self.sink),
            err_root: self.err_root.clone(),
        })
    }
}

impl ignore::ParallelVisitor for CollectVisitor {
    /// This function loops over all matches, ignores directories, and ignores PermissionDenied and
    /// NotFound errors
    fn visit(&mut self, dir_entry: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
        match dir_entry {
            Ok(dir_entry) => {
                if dir_entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    return ignore::WalkState::Continue;
                }
                self.local.push(Ok(dir_entry));
            }
            Err(e) => {
                if let Some(ioe) = e.io_error() {
                    match ioe.kind() {
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {}
                        _ => self
                            .local
                            .push(Err(GlobSetError::Walk(self.err_root.clone(), e))),
                    }
                } else {
                    self.local
                        .push(Err(GlobSetError::Walk(self.err_root.clone(), e)));
                }
            }
        }
        ignore::WalkState::Continue
    }
}

/// Walk over the globs in the specific root
pub fn walk_globs(
    effective_walk_root: &Path,
    globs: &[SimpleGlob],
) -> Result<Vec<ignore::DirEntry>, GlobSetError> {
    let mut ob = ignore::overrides::OverrideBuilder::new(effective_walk_root);
    let glob_patterns = globs
        .iter()
        .map(|g| anchor_literal_pattern(g.to_pattern()))
        .collect_vec();

    // Always add ignore hidden folders unless the user explicitly included them
    // because we add patterns as overrides, which overrides any `WalkBuilder` settings.
    let ignore_patterns = ignore_hidden_folders(glob_patterns);

    for pattern in ignore_patterns {
        ob.add(&pattern).map_err(GlobSetError::BuildOverrides)?;
    }

    let overrides = ob.build().map_err(GlobSetError::BuildOverrides)?;

    let walker = ignore::WalkBuilder::new(effective_walk_root)
        .git_ignore(true)
        .git_exclude(true)
        .hidden(true)
        .git_global(false)
        .ignore(false)
        .overrides(overrides)
        .build_parallel();

    let collected: SharedResults = Arc::new(Mutex::new(Some(Vec::new())));
    let start = std::time::Instant::now();

    let mut builder = CollectBuilder {
        sink: Arc::clone(&collected),
        err_root: effective_walk_root.to_path_buf(),
    };
    walker.visit(&mut builder);

    let results = collected.lock().take().unwrap_or_default();

    // Log some statistics as long as we are unsure with regards to performance
    let matched = results.len();
    let elapsed = start.elapsed();
    let (include, excludes): (Vec<_>, Vec<_>) = globs.iter().partition(|g| !g.is_negated());
    let include_patterns = include.iter().map(|g| g.to_pattern()).join(", ");
    let exclude_patterns = excludes.iter().map(|g| g.to_pattern()).join(", ");

    tracing::debug!(
        include = include_patterns,
        excludes = exclude_patterns,
        matched,
        elapsed_ms = elapsed.as_millis(),
        "glob pass completed"
    );

    results.into_iter().try_collect()
}

/// Ensures plain file names behave as "current directory" matches for the ignore crate.
///
/// Gitignore syntax treats bare literals (e.g. `pixi.toml`) as "match anywhere below the root".
/// To keep parity with the previous wax-based globbing, which treated them like Unix globs anchored
/// to the working directory, we prepend a `/` so the override only applies at the search root.
/// Literals are anchored whether they are positive or negated—`foo` matches only the root file and
/// `!foo` excludes only that file—while anything containing meta characters or directory separators
/// is left untouched and keeps gitignore semantics.
fn anchor_literal_pattern(pattern: String) -> String {
    fn needs_anchor(body: &str) -> bool {
        if body.is_empty() {
            return false;
        }
        // These will not occur when used in conjunction with GlobWalkRoot, but lets keep
        // them for if this is not used in conjunction with these
        if body.starts_with("./") || body.starts_with('/') || body.starts_with("../") {
            return false;
        }
        if body.contains('/') {
            return false;
        }
        if body.chars().any(|c| matches!(c, '*' | '?' | '[' | '{')) {
            return false;
        }
        true
    }

    let (negated, body) = if let Some(rest) = pattern.strip_prefix('!') {
        (true, rest)
    } else {
        (false, pattern.as_str())
    };

    if needs_anchor(body) {
        let mut anchored = String::with_capacity(pattern.len() + 1);
        if negated {
            anchored.push('!');
        }
        anchored.push('/');
        anchored.push_str(body);
        anchored
    } else {
        pattern
    }
}

/// Ensures that hidden folders (starting with a dot) are always ignored unless explicitly included.
/// This is done by adding a negated pattern for `**/.*` unless the user
/// already specified a pattern that would include hidden folders or a specific hidden folder.
/// This is important to avoid accidentally including hidden folders in the results when patterns like ** are used.
/// Also, negated pattern should go after other patterns to ensure they are applied correctly.
pub fn ignore_hidden_folders(mut patterns: Vec<String>) -> Vec<String> {
    // Detect if user explicitly included hidden folders *globally*
    // e.g. ".*", "**/.*", "./.*", "*/.*", etc.
    let user_includes_hidden_globally = patterns
        .iter()
        .any(|p| p == ".*" || p == "./.*" || p.contains("/.*"));

    if user_includes_hidden_globally {
        return patterns;
    }

    // Append negations at the end if they aren't already present.
    if !patterns.iter().any(|p| p == "!.*") {
        patterns.push("!.*".to_string());
    }
    if !patterns.iter().any(|p| p == "!**/.*") {
        patterns.push("!**/.*".to_string());
    }

    patterns
}

#[cfg(test)]
mod tests {
    use crate::glob_set::walk::ignore_hidden_folders;

    use super::anchor_literal_pattern;

    #[test]
    fn anchors_literal_file_patterns() {
        assert_eq!(anchor_literal_pattern("pixi.toml".to_string()), "pixi.toml");
        // Patterns that already specify a subdirectory should stay untouched.
        assert_eq!(
            anchor_literal_pattern("foo/bar/baz.txt".to_string()),
            "foo/bar/baz.txt"
        );
    }

    #[test]
    fn leaves_non_literal_patterns_untouched() {
        assert_eq!(
            anchor_literal_pattern("!pixi.toml".to_string()),
            "!pixi.toml"
        );
        assert_eq!(anchor_literal_pattern("*.toml".to_string()), "*.toml");
        assert_eq!(anchor_literal_pattern("!*.toml".to_string()), "!*.toml");
        assert_eq!(
            anchor_literal_pattern("src/lib.rs".to_string()),
            "src/lib.rs"
        );
        assert_eq!(
            anchor_literal_pattern("../pixi.toml".to_string()),
            "../pixi.toml"
        );
    }

    #[test]
    fn adds_negated_patterns_when_no_hidden_includes() {
        let input = vec!["**".to_string()];
        let expected = vec!["**".to_string(), "!.*".to_string(), "!**/.*".to_string()];
        assert_eq!(ignore_hidden_folders(input), expected);
    }

    #[test]
    fn explicit_hidden_include_is_kept_and_negated_patterns_added_at_end() {
        let input = vec!["**".to_string(), ".nichita".to_string()];
        let expected = vec![
            "**".to_string(),
            ".nichita".to_string(),
            "!.*".to_string(),
            "!**/.*".to_string(),
        ];
        assert_eq!(ignore_hidden_folders(input), expected);
    }

    // #[test]
    // fn global_hidden_include_skips_negations() {
    //     let input = vec!["**".into(), "**/.*".into()];
    //     let expected = vec!["**".into(), "**/.*".into()];
    //     assert_eq!(ignore_hidden_folders(input), expected);
    // }

    // #[test]
    // fn deduplicates_auto_negations() {
    //     let input = vec!["**".into(), "!.*".into()];
    //     let expected = vec!["**".into(), "!.*".into(), "!**/.*".into()];
    //     assert_eq!(ignore_hidden_folders(input), expected);
    // }

    // #[test]
    // fn no_negations_if_hidden_included_globally() {
    //     let input = vec!["**".into(), "./.*".into()];
    //     let expected = vec!["**".into(), "./.*".into()];
    //     assert_eq!(ignore_hidden_folders(input), expected);
    // }

    // #[test]
    // fn handles_multiple_includes() {
    //     let input = vec!["**".into(), ".git".into(), ".env".into()];
    //     let expected = vec![
    //         ".git".into(),
    //         ".env".into(),
    //         "**".into(),
    //         ".git".into(),
    //         ".env".into(),
    //         "!.*".into(),
    //         "!**/.*".into(),
    //     ];
    //     assert_eq!(ignore_hidden_folders(input), expected);
    // }

    // #[test]
    // fn works_with_empty_input() {
    //     let input: Vec<String> = vec![];
    //     let expected = vec!["!.*".into(), "!**/.*".into()];
    //     assert_eq!(ignore_hidden_folders(input), expected);
    // }
}
