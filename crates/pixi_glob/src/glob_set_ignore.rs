use std::path::{Component, Path, PathBuf};

use itertools::Itertools;
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// A glob set implemented using the `ignore` crate (globset + fast walker).
pub struct GlobSetIgnore<'t> {
    /// Include patterns (gitignore-style), without leading '!'.
    pub include: Vec<&'t str>,
    /// Exclude patterns (gitignore-style), without leading '!'.
    pub exclude: Vec<&'t str>,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum GlobSetIgnoreError {
    #[error("failed to build overrides")]
    BuildOverrides(#[source] ignore::Error),

    #[error("walk error at {0}")]
    Walk(PathBuf, #[source] ignore::Error),
}

impl<'t> GlobSetIgnore<'t> {
    /// Create a new `GlobSetIgnore` from a list of patterns. Leading '!' indicates exclusion.
    pub fn create(globs: impl IntoIterator<Item = &'t str>) -> GlobSetIgnore<'t> {
        let (include, exclude): (Vec<_>, Vec<_>) =
            globs.into_iter().partition(|g| !g.starts_with('!'));

        let exclude = exclude.into_iter().map(|g| &g[1..]).collect_vec();
        GlobSetIgnore { include, exclude }
    }

    /// Walks files matching all include/exclude patterns using a single parallel walker.
    /// Returns a flat Vec of results to keep lifetimes simple and predictable.
    pub fn collect_matching(
        &self,
        root_dir: &Path,
    ) -> Result<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>, GlobSetIgnoreError> {
        // Prepare include roots and relative patterns.
        let prepared: Vec<(PathBuf, String)> = self
            .include
            .iter()
            .map(|&inc| split_pattern_prefix(root_dir, inc))
            .collect();

        if prepared.is_empty() {
            return Ok(Vec::new());
        }

        // Compute a common ancestor for all include roots to walk exactly once.
        let common_root = common_ancestor(prepared.iter().map(|(r, _)| r));

        // Build one overrides set with all includes and excludes adjusted relative to `common_root`.
        let mut ob = ignore::overrides::OverrideBuilder::new(&common_root);
        for (walk_root, rel_pat) in &prepared {
            let prefix = walk_root
                .strip_prefix(&common_root)
                .unwrap_or(Path::new(""));
            let prefix_str = if prefix.as_os_str().is_empty() {
                String::new()
            } else {
                prefix.to_string_lossy().replace('\\', "/")
            };
            let mut inc_pat = if prefix_str.is_empty() {
                rel_pat.clone()
            } else if rel_pat.is_empty() {
                prefix_str.clone()
            } else {
                format!("{}/{}", prefix_str, rel_pat)
            };
            if inc_pat.is_empty() {
                inc_pat = String::from("**/*");
            }
            ob.add(&inc_pat)
                .map_err(GlobSetIgnoreError::BuildOverrides)?;

            // Add excludes for this include context
            for &ex in &self.exclude {
                let ex_pat = if prefix_str.is_empty() {
                    format!("!{}", ex)
                } else {
                    format!("!{}/{}", prefix_str, ex)
                };
                ob.add(&ex_pat)
                    .map_err(GlobSetIgnoreError::BuildOverrides)?;
            }
        }
        let overrides = ob.build().map_err(GlobSetIgnoreError::BuildOverrides)?;

        // Single parallel walk.
        let walker = ignore::WalkBuilder::new(&common_root)
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .hidden(false)
            .overrides(overrides)
            .build_parallel();

        let collected: Arc<Mutex<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let collected_ref = Arc::clone(&collected);

        let start = std::time::Instant::now();
        walker.run(|| {
            let collected = Arc::clone(&collected_ref);
            let root_for_err = common_root.clone();
            Box::new(move |dent| {
                match dent {
                    Ok(dent) => {
                        if dent.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                            return ignore::WalkState::Continue;
                        }
                        if let Ok(mut guard) = collected.lock() {
                            guard.push(Ok(dent));
                        }
                    }
                    Err(e) => {
                        if let Some(ioe) = e.io_error() {
                            match ioe.kind() {
                                std::io::ErrorKind::NotFound
                                | std::io::ErrorKind::PermissionDenied => {}
                                _ => {
                                    if let Ok(mut guard) = collected.lock() {
                                        guard.push(Err(GlobSetIgnoreError::Walk(
                                            root_for_err.clone(),
                                            e,
                                        )));
                                    }
                                }
                            }
                        } else if let Ok(mut guard) = collected.lock() {
                            guard.push(Err(GlobSetIgnoreError::Walk(
                                root_for_err.clone(),
                                e,
                            )));
                        }
                    }
                }
                ignore::WalkState::Continue
            })
        });

        // Drain results and log total timing
        let mut results = Vec::new();
        if let Ok(mut guard) = collected.lock() {
            let matched = guard.len();
            results.extend(guard.drain(..));
            let elapsed = start.elapsed();
            tracing::info!(
                includes = prepared.len(),
                matched,
                elapsed_ms = elapsed.as_millis(),
                "merged glob walk completed (ignore)"
            );
        }

        Ok(results)
    }
}

/// Split a pattern into a concrete path prefix (to adjust the walk root)
/// and the remaining pattern relative to that prefix. This approximates the
/// behavior of `wax`'s semantic literal partitioning.
fn split_pattern_prefix(root_dir: &Path, pattern: &str) -> (PathBuf, String) {
    // Normalize separators to platform-specific for splitting.
    // We'll treat both '/' and '\\' as separators to be safe.
    let sep_normalized = pattern.replace('\\', "/");
    let mut parts = sep_normalized.split('/').peekable();

    let mut prefix = PathBuf::new();
    let mut consumed = 0usize;

    while let Some(&seg) = parts.peek() {
        // Stop on glob meta in this segment
        let has_meta =
            seg.contains('*') || seg.contains('?') || seg.contains('[') || seg.contains('{');
        if seg == "" || seg == "." || has_meta {
            break;
        }
        // Consume this concrete path segment (may be "..")
        let _ = parts.next();
        consumed += 1;
        match seg {
            "." => {}
            _ => prefix.push(seg),
        }
    }

    // Remainder after the consumed prefix segments
    let remainder = sep_normalized
        .split('/')
        .skip(consumed)
        .collect::<Vec<_>>()
        .join("/");
    let remainder = if remainder.is_empty() {
        String::from("**/*")
    } else {
        remainder
    };

    let walk_root = if prefix.as_os_str().is_empty() {
        root_dir.to_path_buf()
    } else {
        normalize_join(root_dir, &prefix)
    };
    (walk_root, remainder)
}

fn normalize_join(base: &Path, rel: &Path) -> PathBuf {
    let mut out = PathBuf::from(base);
    for comp in rel.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(comp.as_os_str()),
        }
    }
    out
}

/// Compute the deepest common ancestor path of an iterator of paths.
fn common_ancestor<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> PathBuf {
    let mut it = paths.into_iter();
    let mut prefix: Vec<_> = match it.next() {
        Some(p) => p.components().collect(),
        None => return PathBuf::new(),
    };
    for p in it {
        let mut comps = p.components();
        let mut i = 0usize;
        while i < prefix.len() {
            match comps.next() {
                Some(c) if c == prefix[i] => i += 1,
                _ => break,
            }
        }
        prefix.truncate(i);
        if prefix.is_empty() {
            break;
        }
    }
    let mut out = PathBuf::new();
    for c in prefix {
        out.push(c.as_os_str());
    }
    out
}
