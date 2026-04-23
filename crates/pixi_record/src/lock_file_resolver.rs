//! Index-based lookup of [`UnresolvedPixiRecord`]s for every package in a
//! [`LockFile`].
//!
//! Building a source record requires access to its `build_packages` and
//! `host_packages` as [`UnresolvedPixiRecord`] values. The lockfile stores
//! those as index sets; this resolver walks the full package table once in
//! topological order and produces an `UnresolvedPixiRecord` per entry with
//! `build_packages` / `host_packages` populated from already-built
//! predecessors.

use std::path::Path;

use rattler_lock::{LockFile, LockedPackage, PackageHandle};
use thiserror::Error;

use crate::{ParseLockFileError, UnresolvedPixiRecord};

/// Maps every package in a [`LockFile`] to an `UnresolvedPixiRecord`.
///
/// Conda packages are represented. Pypi packages occupy a slot so that
/// positional indexing lines up with [`LockFile::packages`], but the slot is
/// `None`.
#[derive(Debug)]
pub struct LockFileResolver {
    records: Vec<Option<UnresolvedPixiRecord>>,
    /// Start address of the source lockfile's `packages()` slice, stored as
    /// a `usize` so the struct stays `Send + Sync`. Used to map an incoming
    /// `&LockedPackage` back to its index via pointer-offset arithmetic —
    /// same fragility as any pointer-identity scheme: callers must pass
    /// references into the same, still-alive lockfile.
    packages_start_addr: usize,
    packages_len: usize,
}

impl LockFileResolver {
    /// Build a resolver from every package in `lock_file`. Source records
    /// have their `build_packages` and `host_packages` populated with the
    /// corresponding records from the same resolver.
    ///
    /// The traversal is a DFS post-order over the build/host dependency
    /// graph, so each record is constructed once with its final state. A
    /// cycle in the graph (which shouldn't occur for a valid build DAG)
    /// surfaces as [`LockFileResolverError::Cycle`].
    pub fn build(
        lock_file: &LockFile,
        workspace_root: &Path,
    ) -> Result<Self, LockFileResolverError> {
        let packages = lock_file.packages();
        let packages_start_addr = packages.as_ptr() as usize;
        let packages_len = packages.len();
        let mut records: Vec<Option<UnresolvedPixiRecord>> =
            (0..packages_len).map(|_| None).collect();

        #[derive(Clone, Copy, PartialEq, Eq)]
        enum State {
            Unvisited,
            Visiting,
            Done,
        }
        let mut state = vec![State::Unvisited; packages.len()];

        // Iterative DFS post-order. The stack frame is
        // `(node, deps, cursor)`; when `cursor == deps.len()` we've processed
        // every dependency and can emit the node.
        for root in 0..packages.len() {
            if state[root] != State::Unvisited {
                continue;
            }
            let mut stack: Vec<(usize, Vec<usize>, usize)> = Vec::new();
            let deps = dep_indices(&packages[root]);
            stack.push((root, deps, 0));
            state[root] = State::Visiting;

            while let Some((_, deps, cursor)) = stack.last_mut() {
                if *cursor < deps.len() {
                    let next = deps[*cursor];
                    *cursor += 1;
                    match state[next] {
                        State::Done => {}
                        State::Visiting => {
                            return Err(LockFileResolverError::Cycle(format_cycle(
                                &stack, next, packages,
                            )));
                        }
                        State::Unvisited => {
                            state[next] = State::Visiting;
                            let next_deps = dep_indices(&packages[next]);
                            stack.push((next, next_deps, 0));
                        }
                    }
                } else {
                    let (node, _, _) = stack.pop().expect("stack is non-empty in this branch");
                    state[node] = State::Done;
                    records[node] = build_record(&packages[node], workspace_root, &records)?;
                }
            }
        }

        Ok(Self {
            records,
            packages_start_addr,
            packages_len,
        })
    }

    /// Returns the resolver's record for a package. The reference must point
    /// into the same lockfile that built the resolver — references from
    /// clones or other lockfiles yield `None`.
    pub fn get_for_package(&self, package: &LockedPackage) -> Option<UnresolvedPixiRecord> {
        let addr = package as *const LockedPackage as usize;
        let offset = addr.checked_sub(self.packages_start_addr)?;
        let stride = size_of::<LockedPackage>();
        if offset % stride != 0 {
            return None;
        }
        let idx = offset / stride;
        if idx >= self.packages_len {
            return None;
        }
        self.records[idx].clone()
    }
}

/// Construct the final `UnresolvedPixiRecord` for a package. For conda source
/// packages, `build_packages` and `host_packages` are populated from
/// already-built slots in `records`.
fn build_record(
    locked: &LockedPackage,
    workspace_root: &Path,
    records: &[Option<UnresolvedPixiRecord>],
) -> Result<Option<UnresolvedPixiRecord>, LockFileResolverError> {
    let LockedPackage::Conda(data) = locked else {
        return Ok(None);
    };

    let (build_packages, host_packages) = match data.as_source() {
        Some(source) => (
            collect_records(records, source.source_data.build_packages.raw_handles()),
            collect_records(records, source.source_data.host_packages.raw_handles()),
        ),
        None => (Vec::new(), Vec::new()),
    };

    let unresolved = UnresolvedPixiRecord::from_conda_package_data(
        data.clone(),
        workspace_root,
        build_packages,
        host_packages,
    )
    .map_err(LockFileResolverError::Parse)?;
    Ok(Some(unresolved))
}

/// Collect an iterator of handles into a vector of `UnresolvedPixiRecord`,
/// skipping any handle that resolves to `None` (i.e. a pypi package or,
/// defensively, an out-of-bounds slot).
fn collect_records<'a>(
    records: &[Option<UnresolvedPixiRecord>],
    handles: impl IntoIterator<Item = &'a PackageHandle>,
) -> Vec<UnresolvedPixiRecord> {
    handles
        .into_iter()
        .filter_map(|h| records.get(h.as_usize())?.clone())
        .collect()
}

/// Build and host dependency indices for a package. Empty for binary and pypi
/// packages.
fn dep_indices(package: &LockedPackage) -> Vec<usize> {
    let Some(source) = package.as_conda().and_then(|c| c.as_source()) else {
        return Vec::new();
    };
    source
        .source_data
        .build_packages
        .raw_handles()
        .chain(source.source_data.host_packages.raw_handles())
        .map(PackageHandle::as_usize)
        .collect()
}

/// Build a human-readable description of the cycle that was just detected.
/// `stack` contains every node currently being visited (from the DFS root down
/// to the caller of the back-edge), and `back_edge_target` is the node the
/// back-edge points to — i.e. the node that closes the cycle. Walks the stack
/// from that target to the top and formats the names as `a -> b -> c -> a`.
fn format_cycle(
    stack: &[(usize, Vec<usize>, usize)],
    back_edge_target: usize,
    packages: &[LockedPackage],
) -> String {
    let mut names: Vec<&str> = stack
        .iter()
        .skip_while(|(n, _, _)| *n != back_edge_target)
        .map(|(n, _, _)| packages[*n].name())
        .collect();
    names.push(packages[back_edge_target].name());
    names.join(" -> ")
}

#[derive(Debug, Error)]
pub enum LockFileResolverError {
    #[error(transparent)]
    Parse(#[from] ParseLockFileError),

    #[error("the lockfile's build/host package graph contains a cycle: {0}")]
    Cycle(String),
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rattler_lock::LockFile;

    use super::{LockFileResolver, LockFileResolverError};

    /// Two source packages that list each other as a build dependency.
    /// `LockFileResolver::build` should detect the back-edge during DFS
    /// and surface a `Cycle` error whose payload names both packages.
    #[test]
    fn cycle_error_reports_path() {
        let lock_source = r#"version: 7
platforms:
- name: noarch
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      noarch:
      - conda_source: foo[11111111] @ git+https://github.com/example/foo.git?tag=v1#0000000000000000000000000000000000000001
      - conda_source: bar[22222222] @ git+https://github.com/example/bar.git?tag=v1#0000000000000000000000000000000000000002
packages:
- conda_source: foo[11111111] @ git+https://github.com/example/foo.git?tag=v1#0000000000000000000000000000000000000001
  version: 1.0.0
  build: h0
  subdir: noarch
  build_packages:
  - conda_source: bar[22222222] @ git+https://github.com/example/bar.git?tag=v1#0000000000000000000000000000000000000002
- conda_source: bar[22222222] @ git+https://github.com/example/bar.git?tag=v1#0000000000000000000000000000000000000002
  version: 2.0.0
  build: h0
  subdir: noarch
  build_packages:
  - conda_source: foo[11111111] @ git+https://github.com/example/foo.git?tag=v1#0000000000000000000000000000000000000001
"#;

        let lock_file =
            LockFile::from_str_with_base_directory(lock_source, Some(Path::new("/workspace")))
                .expect("cyclic lockfile should still parse");

        let err = LockFileResolver::build(&lock_file, Path::new("/workspace"))
            .expect_err("resolver should reject a cyclic build graph");

        let LockFileResolverError::Cycle(path) = err else {
            panic!("expected Cycle error, got {err:?}");
        };
        let segments: Vec<&str> = path.split(" -> ").collect();
        assert!(
            segments.len() >= 3,
            "cycle path should have at least start -> mid -> start: {path}"
        );
        assert_eq!(
            segments.first(),
            segments.last(),
            "cycle path should close on itself: {path}"
        );
        assert!(path.contains("foo"), "cycle path should mention foo: {path}");
        assert!(path.contains("bar"), "cycle path should mention bar: {path}");
    }
}
