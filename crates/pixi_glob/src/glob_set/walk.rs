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
    let ignore_patterns = set_ignore_hidden_patterns(&glob_patterns);

    for provided_pattern in &glob_patterns {
        ob.add(provided_pattern)
            .map_err(GlobSetError::BuildOverrides)?;
    }

    let enable_ignoring_hidden = if let Some(ref patterns) = ignore_patterns {
        // If we added negated patterns for hidden folders, we want to allow searching through hidden folders
        // unless the user explicitly included them
        tracing::debug!("Adding ignore patterns for hidden folders: {:?}", patterns);
        for pattern in patterns {
            ob.add(pattern).map_err(GlobSetError::BuildOverrides)?;
        }
        false
    } else {
        true
    };

    let overrides = ob.build().map_err(GlobSetError::BuildOverrides)?;

    let mut builder = ignore::WalkBuilder::new(effective_walk_root);

    let walker_builder = builder
        .git_ignore(true)
        .git_exclude(true)
        .hidden(enable_ignoring_hidden)
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
    walker_builder.visit(&mut builder);

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
        root = ?effective_walk_root,
        "glob pass completed"
    );

    results.into_iter().collect()
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

/// Ensures that hidden folders (starting with a dot) are always ignored unless explicitly included.
/// The ones that are requested are added back as a whitelist.
pub fn set_ignore_hidden_patterns(patterns: &[String]) -> Option<Vec<String>> {
    // Detect if user explicitly included hidden folders
    // e.g. ".*", "**/.*", ".foobar/*", "**/.deep_hidden/**", etc.
    let user_includes_hidden = patterns.iter().any(|p| {
        // Check if pattern starts with a dot (whitelist)
        p.starts_with('.') ||
            // Check if pattern contains a hidden folder path component
            p.contains("/.") && !p.starts_with("!.")
    });

    // Check if negation patterns for all hidden files/folders already exist
    let has_negation_for_all_folders = patterns.iter().any(|p| p.starts_with("!**/.*"));

    let requested_everything = patterns
        .iter()
        .any(|p| p == "**" || p == "./**" || p == "**/*" || p == "./**/*");

    if has_negation_for_all_folders {
        // If user negated all hidden folders, we do not need to add anything
        return None;
    }

    let search_all_hidden = patterns
        .iter()
        .any(|p| p == ".*" || p == ".**" || p == "**/.*" || p == "./.*" || p == ".**/*");

    // If user requested searching through hidden folders,
    // we allow searching them all and don't add any negation patterns
    if search_all_hidden {
        return patterns.to_vec().into();
    }

    // If user has explicitly included hidden folders and no negation exists,
    // add the negation pattern at the end, then whitelist specific folders
    // Example:
    // Input: ["**", ".foo/bar.txt"]
    // Output: ["**", ".foo", "!{**/.*, .*, .**/*}", ".foo", "!.foo/*", ".foo/bar.txt"]
    // This is because `ignore` globs work as a whitelist ignore
    // so first, we need to ignore all hidden files/folders,
    // then add back the requested ones ( just the folder name, for some reason we don't know why .foo/bar.txt doesn't work )
    // then ignore all its contents, then add back the specific file.
    // This is a special case only when the user asks for all folders/files ( ** glob), which overrides all WalkBuilder settings
    // or user requested hidden folders explicitly
    if requested_everything || (user_includes_hidden && !has_negation_for_all_folders) {
        let mut result = patterns.to_vec();
        let mut seen = std::collections::HashSet::new();

        // Track which patterns we've already added
        for p in patterns {
            seen.insert(p.clone());
        }

        result.push("!{**/.*, .*, .**/*}".to_string());
        seen.insert("!{**/.*, .*, .**/*}".to_string());

        // Now add back any explicitly whitelisted hidden folders/files
        for pattern in patterns {
            if (pattern.starts_with('.') || pattern.contains("/.")) && !pattern.starts_with("!.") {
                // Check if this is a specific file path (not a glob pattern)
                let is_specific_file = !pattern.contains('*')
                    && !pattern.contains('?')
                    && !pattern.contains('[')
                    && pattern.contains('/');

                if is_specific_file {
                    // Transform specific file paths: .nichita/foo.txt
                    if let Some(last_slash) = pattern.rfind('/') {
                        let dir = &pattern[..last_slash];

                        // Add: directory, negation of all its contents, then the specific file
                        if seen.insert(dir.to_string()) {
                            result.push(dir.to_string());
                        }

                        let negate_all = format!("!{}/*", dir);
                        if seen.insert(negate_all.clone()) {
                            result.push(negate_all);
                        }

                        // Always re-add the specific file pattern at the end
                        result.push(pattern.clone());
                    }
                } else {
                    // Extract the hidden folder name from patterns like:
                    // ".pixi/*" -> ".pixi"
                    // "**/.deep_pixi/**" -> ".deep_pixi"
                    let hidden_folder = if pattern.starts_with('.') {
                        // Pattern like ".pixi/*"
                        pattern
                    } else if let Some(idx) = pattern.find("/.") {
                        // Pattern like "**/.deep_pixi/**"
                        let after_slash = &pattern[idx + 1..];
                        after_slash.split('/').next().unwrap_or(pattern)
                    } else {
                        continue;
                    };

                    // Re-add the whitelisted folder and its contents
                    if seen.insert(hidden_folder.to_string()) {
                        result.push(hidden_folder.to_string());
                    }
                }
            }
        }

        return Some(result);
    }

    None
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

    // #[test]
    // fn adds_negated_patterns_when_no_hidden_includes() {
    //     let input = vec!["**".to_string()];
    //     let expected = vec!["**".to_string(), "!.*".to_string(), "!**/.*".to_string()];
    //     assert_eq!(ignore_hidden_patterns(input), expected);
    // }

    // #[test]
    // fn explicit_hidden_include_is_kept_and_negated_patterns_added_at_end() {
    //     let input = vec!["**".to_string(), ".nichita".to_string()];
    //     let expected = vec![
    //         "**".to_string(),
    //         ".nichita".to_string(),
    //         "!.*".to_string(),
    //         "!**/.*".to_string(),
    //     ];
    //     assert_eq!(ignore_hidden_patterns(input), expected);
    // }
}
