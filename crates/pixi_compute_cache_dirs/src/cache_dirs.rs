//! [`CacheDirs`] and the [`CacheDirsKey`] injection point.

use std::{
    any::TypeId,
    collections::HashMap,
    fmt::{self, Display, Formatter},
    sync::Arc,
};

use pixi_compute_engine::InjectedKey;
use pixi_path::{AbsPathBuf, AbsPresumedDirPath, AbsPresumedDirPathBuf};

use crate::CacheLocation;

/// Anchor that a cache root resolves against.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CacheBase {
    /// Global cache root (e.g. `~/.cache/pixi`).
    Root,
    /// Workspace cache root (e.g. `<workspace>/.pixi`); falls back to
    /// [`CacheBase::Root`] when no workspace is configured.
    Workspace,
}

/// Anchor paths and per-type overrides for the engine's caches.
///
/// Held inside [`CacheDirsKey`]; the actual per-type cache path is
/// resolved through [`crate::CacheDirKey`].
#[derive(Clone, Debug)]
pub struct CacheDirs {
    root: AbsPresumedDirPathBuf,
    workspace: Option<AbsPresumedDirPathBuf>,
    overrides: HashMap<TypeId, AbsPresumedDirPathBuf>,
}

impl CacheDirs {
    /// Build a `CacheDirs` rooted at `root`. Workspace and overrides are
    /// unset by default.
    pub fn new(root: AbsPresumedDirPathBuf) -> Self {
        Self {
            root,
            workspace: None,
            overrides: HashMap::new(),
        }
    }

    /// Set the workspace anchor.
    pub fn with_workspace(self, workspace: AbsPresumedDirPathBuf) -> Self {
        Self {
            workspace: Some(workspace),
            ..self
        }
    }

    /// Override the resolved path for `L`. Wins over the env-var fallback
    /// inside [`crate::CacheDirKey`].
    pub fn with_override<L: CacheLocation>(mut self, path: AbsPresumedDirPathBuf) -> Self {
        self.overrides.insert(TypeId::of::<L>(), path);
        self
    }

    /// Mutating equivalent of [`Self::with_override`].
    pub fn set_override<L: CacheLocation>(&mut self, path: AbsPresumedDirPathBuf) {
        self.overrides.insert(TypeId::of::<L>(), path);
    }

    /// Global cache root.
    pub fn root(&self) -> &AbsPresumedDirPath {
        &self.root
    }

    /// Workspace anchor when set.
    pub fn workspace(&self) -> Option<&AbsPresumedDirPath> {
        self.workspace.as_deref()
    }

    /// Resolve `base` to a concrete path. Workspace falls back to
    /// [`Self::root`] when no workspace is configured.
    pub fn anchor(&self, base: CacheBase) -> AbsPresumedDirPathBuf {
        match base {
            CacheBase::Root => self.root.clone(),
            CacheBase::Workspace => self.workspace.clone().unwrap_or_else(|| self.root.clone()),
        }
    }

    /// Programmatic override for `L`, if registered.
    pub fn override_for<L: CacheLocation>(&self) -> Option<&AbsPresumedDirPathBuf> {
        self.overrides.get(&TypeId::of::<L>())
    }

    /// Resolve `L`'s path synchronously using `env_get` for the optional
    /// env-var lookup. Use outside Key compute bodies (CLI, builder);
    /// inside a compute body use
    /// [`CacheDirsExt::cache_dir`](crate::CacheDirsExt::cache_dir) so
    /// the dependency on the env var is recorded in the engine graph.
    ///
    /// Resolution order matches [`crate::CacheDirKey`]:
    /// override -> env var -> `<base>/<name>`.
    pub fn resolve<L: CacheLocation>(
        &self,
        env_get: impl Fn(&str) -> Option<String>,
    ) -> AbsPresumedDirPathBuf {
        if let Some(p) = self.override_for::<L>() {
            return p.clone();
        }
        if let Some(name) = L::env_override()
            && let Some(raw) = env_get(name)
            && let Ok(p) = AbsPathBuf::new(raw)
        {
            return p.into_assume_dir();
        }
        self.anchor(L::base()).join(L::name()).into_assume_dir()
    }

    /// Convenience over [`Self::resolve`] that pulls env var values
    /// from a previously-fetched [`HashMap`]. Use when the caller has
    /// already snapshotted the env (e.g. via
    /// `engine.read(&EnvVarsKey)`).
    pub fn resolve_with_env<L: CacheLocation>(
        &self,
        env: &HashMap<String, String>,
    ) -> AbsPresumedDirPathBuf {
        self.resolve::<L>(|n| env.get(n).cloned())
    }

    /// Convenience over [`Self::resolve`] that reads env var overrides
    /// from the current process environment via [`std::env::var`]. Use
    /// for sync sites that have no engine snapshot to consult, such as
    /// CLI commands that just need a path.
    pub fn resolve_from_env<L: CacheLocation>(&self) -> AbsPresumedDirPathBuf {
        self.resolve::<L>(|n| std::env::var(n).ok())
    }
}

/// Injected handle to the engine's [`CacheDirs`].
///
/// Read directly via
/// [`ComputeEngine::read`](pixi_compute_engine::ComputeEngine::read) from
/// synchronous code, or depend on it inside a Key compute body via
/// [`ctx.compute(&CacheDirsKey)`](pixi_compute_engine::ComputeCtx::compute).
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct CacheDirsKey;

impl Display for CacheDirsKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("CacheDirs")
    }
}

impl InjectedKey for CacheDirsKey {
    type Value = Arc<CacheDirs>;
}
