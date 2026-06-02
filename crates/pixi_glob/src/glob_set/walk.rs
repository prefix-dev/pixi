//! Contains the directory walking implementation
use itertools::Itertools;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::glob_set::walk_root::SimpleGlob;

use super::{GlobSetError, Match};

type SharedResults = Arc<Mutex<Option<Vec<Result<Match, GlobSetError>>>>>;

struct CollectBuilder {
    // Shared aggregation storage wrapped in an Option so we can `take` at the end.
    sink: SharedResults,
    // The root we are walking, used for error reporting
    err_root: PathBuf,
    // When false, drop file entries the walker yields — used for leaf-only
    // walks where the override is empty and every file would otherwise pass.
    collect_patterns: bool,
}

struct CollectVisitor {
    // Local per-thread buffer to append results without holding the lock.
    local: Vec<Result<Match, GlobSetError>>,
    // Reference to the shared sink.
    sink: SharedResults,
    // The root we are walking, used for error reporting
    err_root: PathBuf,
    // Mirror of `CollectBuilder::collect_patterns`.
    collect_patterns: bool,
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
            collect_patterns: self.collect_patterns,
        })
    }
}

impl ignore::ParallelVisitor for CollectVisitor {
    /// Loop over all matches, drop directories, drop NotFound/PermissionDenied races.
    fn visit(&mut self, dir_entry: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
        match dir_entry {
            Ok(dir_entry) => {
                if dir_entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                    return ignore::WalkState::Continue;
                }
                if !self.collect_patterns {
                    // Leaf-only walk: an empty override doesn't suppress
                    // yield, so we drop file entries here. Leaves still
                    // arrive via the side-channel sink set up by
                    // `filter_entry`.
                    return ignore::WalkState::Continue;
                }
                self.local.push(Ok(Match::Pattern(dir_entry)));
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

/// Walk `effective_walk_root` collecting paths that match `globs`.  When
/// `markers` is non-empty, each directory the walker enters is also probed
/// for the presence of those file names; a marker hit is then matched
/// against the same pattern override that drives ordinary glob matching,
/// and resolves to one of:
///
/// - **leaf**: marker matches an include pattern → the marker path is
///   appended to the results and descent stops at this directory;
/// - **prune**: marker matches an exclude (`!`) pattern → the whole
///   subtree is skipped;
/// - **noop**: marker matches nothing → walking continues normally.
pub fn walk_globs(
    effective_walk_root: &Path,
    globs: &[SimpleGlob],
    markers: &[String],
    exclude_hidden: bool,
) -> Result<Vec<Match>, GlobSetError> {
    let mut ob = ignore::overrides::OverrideBuilder::new(effective_walk_root);
    let glob_patterns = globs
        .iter()
        .map(|g| anchor_literal_pattern(g.to_pattern()))
        .collect_vec();

    // When hidden exclusion is enabled, the existing helper inspects the
    // patterns to decide which approach to use: either rely on
    // `WalkBuilder::hidden(true)` outright or, when broad patterns like
    // `**` would otherwise drag hidden entries back in, append explicit
    // negations.  When the caller has opted out we don't do anything here
    // and just leave `WalkBuilder::hidden(false)` to yield everything.
    let ignore_patterns = if exclude_hidden {
        set_ignore_hidden_patterns(&glob_patterns)
    } else {
        None
    };

    for provided_pattern in &glob_patterns {
        ob.add(provided_pattern)
            .map_err(GlobSetError::BuildOverrides)?;
    }

    let walker_hidden_setting = if !exclude_hidden {
        false
    } else if let Some(ref patterns) = ignore_patterns {
        // If we added negated patterns for hidden folders, we want to allow searching through hidden folders
        // unless the user explicitly included them
        tracing::trace!("Adding ignore patterns for hidden folders: {:?}", patterns);
        for pattern in patterns {
            ob.add(pattern).map_err(GlobSetError::BuildOverrides)?;
        }
        false
    } else {
        true
    };

    let overrides = ob.build().map_err(GlobSetError::BuildOverrides)?;

    // `ignore::WalkBuilder::filter_entry` is not invoked on the root entry,
    // so resolve any marker hits there before starting the walk, mirroring
    // the per-directory dispatch logic: a marker that matches an exclude or
    // that matches nothing prunes the subtree; a marker that matches an
    // include records a leaf.
    let mut root_leaves = Vec::new();
    for marker in markers {
        let marker_path = effective_walk_root.join(marker);
        if !marker_path.is_file() {
            continue;
        }
        match overrides.matched(&marker_path, false) {
            ignore::Match::Whitelist(_) if root_leaves.is_empty() => {
                root_leaves.push(Match::Leaf(marker_path));
            }
            ignore::Match::Whitelist(_) => {}
            // Ignore or None both prune.
            _ => return Ok(Vec::new()),
        }
    }
    if !root_leaves.is_empty() {
        return Ok(root_leaves);
    }

    let leaf_sink: Arc<Mutex<Vec<Match>>> = Arc::new(Mutex::new(Vec::new()));

    let mut builder = ignore::WalkBuilder::new(effective_walk_root);
    builder
        .follow_links(true)
        .git_ignore(false)
        .git_exclude(true)
        .hidden(walker_hidden_setting)
        .git_global(false)
        .ignore(false)
        .overrides(overrides.clone());

    if !markers.is_empty() {
        let markers: Vec<String> = markers.to_vec();
        let overrides_for_filter = overrides.clone();
        let leaf_sink_for_filter = Arc::clone(&leaf_sink);
        builder.filter_entry(move |entry| {
            let Some(ft) = entry.file_type() else {
                return false;
            };
            if !ft.is_dir() {
                // Non-directories pass through to the pattern matcher.
                return true;
            }
            // Pass over all markers present in this dir. A marker that
            // matches an exclude (or no pattern at all) prunes the subtree;
            // a marker that matches an include records a leaf. Pruning wins
            // when both fire in the same directory.
            let mut leaf_match: Option<PathBuf> = None;
            for marker in &markers {
                let marker_path = entry.path().join(marker);
                if !marker_path.is_file() {
                    continue;
                }
                match overrides_for_filter.matched(&marker_path, false) {
                    ignore::Match::Whitelist(_) if leaf_match.is_none() => {
                        leaf_match = Some(marker_path);
                    }
                    ignore::Match::Whitelist(_) => {}
                    // Ignore or None both prune.
                    _ => return false,
                }
            }
            if let Some(leaf) = leaf_match {
                leaf_sink_for_filter.lock().push(Match::Leaf(leaf));
                return false;
            }
            true
        });
    }

    let walker = builder.build_parallel();

    let collected: SharedResults = Arc::new(Mutex::new(Some(Vec::new())));
    let start = std::time::Instant::now();

    let mut collect = CollectBuilder {
        sink: Arc::clone(&collected),
        err_root: effective_walk_root.to_path_buf(),
        collect_patterns: !globs.is_empty(),
    };
    walker.visit(&mut collect);

    let mut results: Vec<Match> = collected
        .lock()
        .take()
        .unwrap_or_default()
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;

    let mut leaves = std::mem::take(&mut *leaf_sink.lock());
    if !leaves.is_empty() {
        results.append(&mut leaves);
    }

    // Log some statistics as long as we are unsure with regards to performance
    let matched = results.len();
    let elapsed = start.elapsed();
    let (include, excludes): (Vec<_>, Vec<_>) = globs.iter().partition(|g| !g.is_negated());
    let include_patterns = include.iter().map(|g| g.to_pattern()).join(", ");
    let exclude_patterns = excludes.iter().map(|g| g.to_pattern()).join(", ");

    tracing::trace!(
        include = include_patterns,
        excludes = exclude_patterns,
        markers = ?markers,
        matched,
        elapsed_ms = elapsed.as_millis(),
        root = ?effective_walk_root,
        "glob pass completed"
    );

    Ok(results)
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
/// The initial problem was that when using glob like: `**` ( which means include everything )
/// overrides our `WalkerBuilder` setting, where we explicitly ignore hidden folders.
/// Imagine a user-provided globs like this:
///
/// ```
/// "**", ".foo/bar.txt"
/// ```
/// To make it work, we need first to ignore all hidden folders after users' globs, so it becomes like this:
/// ```
/// "**", ".foo/bar.txt" "!{**/.*, .*, .**/*}"
/// ```
///
/// Then, we need to whitelist the `.foo` folder (treat it as a special glob, we don't know why, just re-adding back `.foo/bar.txt` doesn't work )
/// Ignore everything from foo: `"!.foo/*"`, and then `whitelist` the `.foo/bar.txt` again.
/// So the final globs will look like this:
///
/// ```
/// ["**", ".foo/bar.txt", "!{**/.*, .*, .**/*}", ".foo", "!.foo/*", ".foo/bar.txt"]
/// ```
///
/// This is a special use case, when the user combines ** with some hidden folders.
/// Otherwise ( in case of ** ), we just ignore every hidden folder
/// Or in case of requesting a simple hidden folder, it will search just for it without any additional negation patterns.
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

    tracing::trace!(
        user_includes_hidden,
        has_negation_for_all_folders,
        search_all_hidden,
        requested_everything,
        "Determining hidden folder handling: ",
    );

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

        // result.push("!{**/.*, .*, .**/*}".to_string());
        result.push("!{**/.*, .*, .**/*}".to_string());

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
                        result.push(dir.to_string());

                        let negate_all = format!("!{dir}/*");
                        result.push(negate_all);

                        // Always re-add the specific file pattern at the end
                        result.push(pattern.clone());
                    }
                } else {
                    // Extract the hidden folder name from patterns like:
                    // ".pixi/*" -> ".pixi"
                    // "**/.deep_pixi/**" -> ".deep_pixi"
                    // ".build/CMakeFiles/**" -> ".build"
                    let hidden_folder = if pattern.starts_with('.') {
                        // Pattern like ".pixi/*" or ".build/CMakeFiles/**"
                        // Extract just the first hidden folder component
                        if let Some(slash_idx) = pattern.find('/') {
                            &pattern[..slash_idx]
                        } else {
                            pattern
                        }
                    } else if let Some(idx) = pattern.find("/.") {
                        // Pattern like "**/.deep_pixi/**"
                        let after_slash = &pattern[idx + 1..];
                        if let Some(slash_idx) = after_slash.find('/') {
                            &after_slash[..slash_idx]
                        } else {
                            after_slash.split('/').next().unwrap_or(pattern)
                        }
                    } else {
                        continue;
                    };

                    // Re-add the whitelisted folder and its contents
                    result.push(hidden_folder.to_string());
                }
            }
        }

        return Some(result.into_iter().collect());
    }

    None
}

#[cfg(test)]
mod tests {
    use crate::glob_set::walk::set_ignore_hidden_patterns;

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

    #[test]
    fn adds_negated_patterns_when_no_hidden_includes() {
        let input = vec!["**".to_string()];
        let expected = vec!["**".to_string(), "!{**/.*, .*, .**/*}".to_string()];
        assert_eq!(set_ignore_hidden_patterns(&input), Some(expected));
    }

    #[test]
    fn hidden_folder_is_whitelisted_at_the_end() {
        let input = vec!["**".to_string(), ".nichita".to_string()];
        let expected = vec![
            "**".to_string(),
            ".nichita".to_string(),
            "!{**/.*, .*, .**/*}".to_string(),
            ".nichita".to_string(),
        ];
        assert_eq!(set_ignore_hidden_patterns(&input), Some(expected));
    }
}
