use itertools::Itertools;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::glob_set::walk_roots::SimpleGlobItem;

use super::GlobSetIgnoreError;

struct CollectBuilder {
    // Shared aggregation storage wrapped in an Option so we can `take` at the end.
    sink: Arc<Mutex<Option<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>>>>,
    err_root: PathBuf,
}

struct CollectVisitor {
    // Local per-thread buffer to append results without holding the lock.
    local: Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>,
    // Reference to the shared sink.
    sink: Arc<Mutex<Option<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>>>>,
    err_root: PathBuf,
}

impl Drop for CollectVisitor {
    // This merges the outputs on the drop
    fn drop(&mut self) {
        let mut sink = self.sink.lock();
        sink.get_or_insert_with(Vec::new)
            .extend(self.local.drain(..));
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
    fn visit(&mut self, dent: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
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
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {}
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

pub fn walk_globs(
    effective_walk_root: &Path,
    globs: &[SimpleGlobItem<'_>],
) -> Result<Vec<ignore::DirEntry>, GlobSetIgnoreError> {
    let mut ob = ignore::overrides::OverrideBuilder::new(effective_walk_root);
    for glob in globs {
        let pattern = glob.to_pattern();
        ob.add(&pattern)
            .map_err(GlobSetIgnoreError::BuildOverrides)?;
    }

    let overrides = ob.build().map_err(GlobSetIgnoreError::BuildOverrides)?;

    let walker = ignore::WalkBuilder::new(effective_walk_root)
        .git_ignore(true)
        .git_exclude(true)
        .hidden(true)
        .git_global(false)
        .ignore(false)
        .overrides(overrides)
        .build_parallel();

    let collected: Arc<Mutex<Option<Vec<Result<ignore::DirEntry, GlobSetIgnoreError>>>>> =
        Arc::new(Mutex::new(Some(Vec::new())));
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
    let include_patterns = globs.iter().filter(|g| !g.negated).count();
    let exclude_patterns = globs.len().saturating_sub(include_patterns);
    tracing::debug!(
        includes = include_patterns,
        excludes = exclude_patterns,
        matched,
        elapsed_ms = elapsed.as_millis(),
        "glob pass completed"
    );

    results.into_iter().try_collect()
}
