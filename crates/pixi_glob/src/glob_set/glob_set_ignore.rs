use std::path::{Path, PathBuf};

use itertools::Itertools;
use std::sync::{Arc, Mutex};
use thiserror::Error;

use crate::glob_set::walk_roots::{SimpleGlobItem, WalkRoots};

/// A glob set implemented using the `ignore` crate (globset + fast walker).
pub struct GlobSetIgnore {
    /// Include patterns (gitignore-style), without leading '!'.
    pub walk_roots: WalkRoots,
}

#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum GlobSetIgnoreError {
    #[error("failed to build overrides")]
    BuildOverrides(#[source] ignore::Error),

    #[error("walk error at {0}")]
    Walk(PathBuf, #[source] ignore::Error),
}

impl GlobSetIgnore {
    /// Create a new `GlobSetIgnore` from a list of patterns. Leading '!' indicates exclusion.
    pub fn create<'t>(globs: impl IntoIterator<Item = &'t str>) -> GlobSetIgnore {
        GlobSetIgnore {
            walk_roots: WalkRoots::build(globs).expect("should not fail"),
        }
    }

    /// Walks files matching all include/exclude patterns using a single parallel walker.
    /// Returns a flat Vec of results to keep lifetimes simple and predictable.
    pub fn collect_matching(
        &self,
        root_dir: &Path,
    ) -> Result<Vec<ignore::DirEntry>, GlobSetIgnoreError> {
        if self.walk_roots.is_empty() {
            return Ok(vec![]);
        }

        let mut all_results = Vec::new();

        for walk_root in &self.walk_roots {
            let effective_walk_root = if walk_root.path().as_os_str().is_empty() {
                root_dir.to_path_buf()
            } else {
                root_dir.join(walk_root.path())
            };

            let globs: Vec<_> = walk_root.into_iter().collect();
            if globs.is_empty() {
                continue;
            }

            let mut results: Vec<_> = Self::walk(&effective_walk_root, &globs)?
                .into_iter()
                .try_collect()?;

            all_results.append(&mut results);
        }

        Ok(all_results)
    }

    /// Perform a walk per unique route
    pub fn walk(
        effective_walk_root: &Path,
        globs: &[SimpleGlobItem<'_>],
    ) -> Result<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>, GlobSetIgnoreError> {
        let mut ob = ignore::overrides::OverrideBuilder::new(effective_walk_root);
        for glob in globs {
            let pattern = glob.to_pattern();
            ob.add(&pattern)
                .map_err(GlobSetIgnoreError::BuildOverrides)?;
        }

        let overrides = ob.build().map_err(GlobSetIgnoreError::BuildOverrides)?;

        // Single parallel walk.
        let walker = ignore::WalkBuilder::new(effective_walk_root)
            // Enable repository local ignores
            .git_ignore(true)
            .git_exclude(true)
            .hidden(true)
            // Dont read global ignores and ag and rg ignores
            .git_global(false)
            .ignore(false)
            .overrides(overrides)
            .build_parallel();
        // Implement a custom per-thread visitor to batch results locally,
        // and merge once per thread on Drop.
        struct CollectBuilder {
            sink: Arc<Mutex<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>>>,
            err_root: PathBuf,
        }

        struct CollectVisitor {
            local: Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>,
            sink: Arc<Mutex<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>>>,
            err_root: PathBuf,
        }

        impl Drop for CollectVisitor {
            fn drop(&mut self) {
                if let Ok(mut guard) = self.sink.lock() {
                    guard.extend(self.local.drain(..));
                }
            }
        }

        impl<'s> ignore::ParallelVisitorBuilder<'s> for CollectBuilder {
            fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 's> {
                Box::new(CollectVisitor {
                    local: Vec::new(),
                    sink: Arc::clone(&self.sink),
                    err_root: self.err_root.clone(),
                })
            }
        }

        impl ignore::ParallelVisitor for CollectVisitor {
            fn visit(
                &mut self,
                dent: Result<ignore::DirEntry, ignore::Error>,
            ) -> ignore::WalkState {
                match dent {
                    Ok(dent) => {
                        if dent.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                            return ignore::WalkState::Continue;
                        }
                        self.local.push(Ok(dent));
                    }
                    Err(e) => {
                        if let Some(ioe) = e.io_error() {
                            match ioe.kind() {
                                std::io::ErrorKind::NotFound
                                | std::io::ErrorKind::PermissionDenied => {}
                                _ => self
                                    .local
                                    .push(Err(GlobSetIgnoreError::Walk(self.err_root.clone(), e))),
                            }
                        } else {
                            self.local
                                .push(Err(GlobSetIgnoreError::Walk(self.err_root.clone(), e)));
                        }
                    }
                }
                ignore::WalkState::Continue
            }
        }

        let collected: Arc<Mutex<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let start = std::time::Instant::now();

        let mut builder = CollectBuilder {
            sink: Arc::clone(&collected),
            err_root: effective_walk_root.to_path_buf(),
        };
        walker.visit(&mut builder);

        let mut results = collected.lock().unwrap_or_else(|p| p.into_inner());
        let matched = results.len();
        let elapsed = start.elapsed();
        let include_patterns = globs.iter().filter(|g| !g.negated).count();
        let exclude_patterns = globs.len().saturating_sub(include_patterns);
        tracing::debug!(
            includes = include_patterns,
            excludes = exclude_patterns,
            matched,
            elapsed_ms = elapsed.as_millis(),
            "glob pass completed"
        );

        Ok(std::mem::take(&mut *results))
    }
}

#[cfg(test)]
mod tests {
    use super::GlobSetIgnore;
    use fs_err::{self as fs, File};
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn sorted_paths(entries: Vec<ignore::DirEntry>, root: &std::path::Path) -> Vec<PathBuf> {
        let mut paths: Vec<_> = entries
            .into_iter()
            .map(|entry| entry.path().strip_prefix(root).unwrap().to_path_buf())
            .collect();
        paths.sort();
        paths
    }

    #[test]
    fn collect_matching_inclusion_exclusion() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        File::create(root_path.join("include1.txt")).unwrap();
        File::create(root_path.join("include2.log")).unwrap();
        File::create(root_path.join("exclude.txt")).unwrap();
        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/include_subdir.txt")).unwrap();

        let glob_set = GlobSetIgnore::create(vec!["**/*.txt", "!exclude.txt"]);
        let entries = glob_set.collect_matching(root_path).unwrap();

        let paths = sorted_paths(entries, root_path);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("include1.txt"),
                PathBuf::from("subdir/include_subdir.txt"),
            ]
        );
    }

    #[test]
    fn collect_matching_relative_globs() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();
        let nested_root = root_path.join("workspace");
        fs::create_dir(&nested_root).unwrap();

        fs::create_dir(root_path.join("subdir")).unwrap();
        File::create(root_path.join("subdir/some_inner_source.cpp")).unwrap();

        let glob_set = GlobSetIgnore::create(vec!["../**/*.cpp"]);
        let entries = glob_set.collect_matching(&nested_root).unwrap();

        let paths = sorted_paths(entries, &nested_root);
        assert_eq!(
            paths,
            vec![PathBuf::from("../subdir/some_inner_source.cpp")]
        );
    }

    #[test]
    fn collect_matching_file_glob() {
        let temp_dir = tempdir().unwrap();
        let root_path = temp_dir.path();

        File::create(root_path.join("pixi.toml")).unwrap();

        let glob_set = GlobSetIgnore::create(vec!["pixi.toml"]);
        let entries = glob_set.collect_matching(root_path).unwrap();

        let paths = sorted_paths(entries, root_path);
        assert_eq!(paths, vec![PathBuf::from("pixi.toml")]);
    }
}
