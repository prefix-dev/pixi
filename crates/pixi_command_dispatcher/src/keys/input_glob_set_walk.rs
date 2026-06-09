//! Compute-engine Key that walks the filesystem according to an
//! [`InputGlobSet`].  The walk root is captured as a *resolved* absolute
//! path so two consumers that arrive at the same workspace via different
//! `caller_root + group.root` combinations dedupe naturally through the
//! engine.
//!
//! Used by the cache-freshness and post-RPC `input_files` computations
//! in `build_backend_metadata` to share a single workspace walk across
//! every backend that points at it during one `pixi lock` run.

use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
};

use derive_more::Display;
use pixi_build_types::InputGlobSet;
use pixi_compute_engine::{ComputeCtx, Key};

/// Inputs for [`InputGlobSetWalkKey`].  Holds the resolved walk root and
/// the walker knobs; everything that affects the result is part of the
/// hash so the engine dedupes correctly.
#[derive(Debug, Clone)]
pub struct InputGlobSetWalkSpec {
    /// Resolved absolute walk root.  Constructors that take a relative
    /// path should join it onto the caller-supplied root before stashing
    /// it here, and canonicalize when feasible.
    pub root: PathBuf,
    /// Gitignore-style patterns; order matters for last-match-wins.
    pub patterns: Vec<String>,
    /// Marker filenames; per-directory presence dispatches against
    /// `patterns`.  Order is not semantically meaningful, but we hash a
    /// sorted view for stable identity.
    pub markers: Vec<String>,
    /// Whether the walker skips hidden directories.
    pub exclude_hidden: bool,
}

impl InputGlobSetWalkSpec {
    /// Build a spec from an [`InputGlobSet`] resolved against
    /// `caller_root`.  Mirrors the resolution rule used by
    /// `crate::input_globs::collect_input_files`: absolute
    /// `root` overrides, relative `root` joins, `None` falls back to
    /// `caller_root`.
    pub fn from_group(group: &InputGlobSet, caller_root: &Path) -> Self {
        let resolved = match group.root.as_deref() {
            Some(p) if p.is_absolute() => p.to_path_buf(),
            Some(p) => caller_root.join(p),
            None => caller_root.to_path_buf(),
        };
        Self {
            root: resolved,
            patterns: group.patterns.clone(),
            markers: group.markers.clone(),
            exclude_hidden: group.exclude_hidden,
        }
    }
}

impl PartialEq for InputGlobSetWalkSpec {
    fn eq(&self, other: &Self) -> bool {
        if self.root != other.root
            || self.patterns != other.patterns
            || self.exclude_hidden != other.exclude_hidden
        {
            return false;
        }
        let mut a: Vec<&String> = self.markers.iter().collect();
        let mut b: Vec<&String> = other.markers.iter().collect();
        a.sort();
        b.sort();
        a == b
    }
}

impl Eq for InputGlobSetWalkSpec {}

impl Hash for InputGlobSetWalkSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.root.hash(state);
        self.patterns.hash(state);
        let mut sorted: Vec<&String> = self.markers.iter().collect();
        sorted.sort();
        for m in sorted {
            m.hash(state);
        }
        self.exclude_hidden.hash(state);
    }
}

/// Compute-engine Key for "walk this group from this resolved root."
/// Cached by the engine for the dispatcher's lifetime; the same
/// `(root, patterns, markers, exclude_hidden)` tuple resolves to a single
/// walk no matter how many consumers ask for it.
#[derive(Clone, Debug, Display, Eq, Hash, PartialEq)]
#[display("{}", _0.root.display())]
pub struct InputGlobSetWalkKey(pub Arc<InputGlobSetWalkSpec>);

impl InputGlobSetWalkKey {
    pub fn new(spec: InputGlobSetWalkSpec) -> Self {
        Self(Arc::new(spec))
    }

    pub fn from_group(group: &InputGlobSet, caller_root: &Path) -> Self {
        Self::new(InputGlobSetWalkSpec::from_group(group, caller_root))
    }
}

impl Key for InputGlobSetWalkKey {
    type Value = Result<Arc<Vec<PathBuf>>, Arc<pixi_glob::GlobSetError>>;

    #[tracing::instrument(
        skip_all,
        name = "input-glob-set-walk",
        fields(root = %self.0.root.display()),
    )]
    async fn compute(&self, _ctx: &mut ComputeCtx) -> Self::Value {
        let spec = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let matches = pixi_glob::GlobSet::create(spec.patterns.iter().map(String::as_str))
                .with_ignore_marker_filenames(spec.markers.iter().map(String::as_str))
                .with_exclude_hidden(spec.exclude_hidden)
                .collect_matching(&spec.root)
                .map_err(Arc::new)?;
            Ok(Arc::new(
                matches.into_iter().map(|m| m.into_path()).collect(),
            ))
        })
        .await
        .unwrap_or_else(|err| match err.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            // spawn_blocking only fails when the runtime is shutting down,
            // which from a Key body we treat as the same as a benign empty
            // result: the surrounding compute is about to be cancelled
            // anyway.
            Err(_) => Ok(Arc::new(Vec::new())),
        })
    }
}
