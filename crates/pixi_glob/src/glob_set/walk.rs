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
    for glob in globs {
        let pattern = anchor_literal_pattern(glob.to_pattern());
        ob.add(&pattern).map_err(GlobSetError::BuildOverrides)?;
    }

    let overrides = ob.build().map_err(GlobSetError::BuildOverrides)?;

    let walker = ignore::WalkBuilder::new(effective_walk_root)
        .git_ignore(false)
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
        let mut anchored = String::with_capacity(pattern.len() + 2);
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

#[cfg(test)]
mod tests {
    use super::anchor_literal_pattern;

    #[test]
    fn anchors_literal_file_patterns() {
        assert_eq!(
            anchor_literal_pattern("pixi.toml".to_string()),
            "/pixi.toml"
        );
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
            "!/pixi.toml"
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
}
