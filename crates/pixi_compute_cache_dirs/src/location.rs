//! [`CacheLocation`] trait, the generic [`CacheDirKey<L>`], and the
//! [`CacheDirsExt`] convenience.

use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
    marker::PhantomData,
};

use pixi_compute_engine::{ComputeCtx, Key};
use pixi_compute_env_vars::EnvVar;
use pixi_path::AbsPresumedDirPathBuf;

use crate::{CacheBase, CacheDirsKey};
// `CacheBase` is referenced by [`CacheLocation::base`] return type below.

/// Static description of a single cache directory.
///
/// Implementors are typically zero-sized markers in the domain crate
/// that owns the cache (e.g. `pub struct GitCache;`). The marker's
/// `TypeId` keys programmatic overrides on
/// [`crate::CacheDirs`]; the associated functions feed the resolution
/// rule used by [`CacheDirKey`].
pub trait CacheLocation: 'static {
    /// Path component appended to the resolved base.
    fn name() -> &'static str;

    /// Anchor the cache is rooted at.
    fn base() -> CacheBase;

    /// Optional environment variable that, when set to an absolute path,
    /// overrides the resolved location. The read goes through
    /// [`EnvVar`] so the dependency is tracked per-name.
    fn env_override() -> Option<&'static str> {
        None
    }
}

/// Computed Key that resolves the path of one [`CacheLocation`].
///
/// Resolution order:
///
/// 1. Programmatic override registered via
///    [`CacheDirs::with_override`](crate::CacheDirs::with_override).
/// 2. Environment variable named by [`CacheLocation::env_override`], if
///    set to an absolute path.
/// 3. Default: `<anchor>/<name>` from [`CacheLocation::base`] and
///    [`CacheLocation::name`].
pub struct CacheDirKey<L>(PhantomData<fn() -> L>);

impl<L> CacheDirKey<L> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<L> Default for CacheDirKey<L> {
    fn default() -> Self {
        Self::new()
    }
}

impl<L> Clone for CacheDirKey<L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L> Copy for CacheDirKey<L> {}

impl<L: 'static> Hash for CacheDirKey<L> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::any::TypeId::of::<L>().hash(state);
    }
}

impl<L> PartialEq for CacheDirKey<L> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<L> Eq for CacheDirKey<L> {}

impl<L: CacheLocation> Display for CacheDirKey<L> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "CacheDir({})", L::name())
    }
}

impl<L: CacheLocation> Debug for CacheDirKey<L> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "CacheDirKey<{}>", L::name())
    }
}

impl<L: CacheLocation> Key for CacheDirKey<L> {
    type Value = AbsPresumedDirPathBuf;

    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let dirs = ctx.compute(&CacheDirsKey).await;
        // Fetch the env var only if `L` declares one, so unrelated env
        // vars do not appear as dependencies of this Key.
        let env_value = if let Some(name) = L::env_override() {
            ctx.compute(&EnvVar(name.to_owned())).await
        } else {
            None
        };
        dirs.resolve::<L>(|_| env_value.clone())
    }
}

/// Resolves a [`CacheLocation`] through the engine.
///
/// Equivalent to `ctx.compute(&CacheDirKey::<L>::new()).await`.
pub trait CacheDirsExt {
    fn cache_dir<L: CacheLocation>(
        &mut self,
    ) -> impl std::future::Future<Output = AbsPresumedDirPathBuf> + Send;
}

impl CacheDirsExt for ComputeCtx {
    fn cache_dir<L: CacheLocation>(
        &mut self,
    ) -> impl std::future::Future<Output = AbsPresumedDirPathBuf> + Send {
        self.compute(&CacheDirKey::<L>::new())
    }
}
