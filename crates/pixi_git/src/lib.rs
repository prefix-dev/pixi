//! Thin pixi-flavoured wrapper around [`rattler_git`].
//!
//! The actual git database/checkout implementation (fetching, LFS,
//! submodules, credentials, the on-disk cache layout) lives in the
//! `rattler_git` crate, which is shared with rattler-build. This crate
//! re-exports it and adds the pixi-specific pieces: the `PIXI_GIT_LFS`
//! environment variable and the default [`CheckoutOptions`] pixi uses.

pub use rattler_git::*;

/// Tri-state default for LFS fetching. Accepts `1`/`0`, `true`/`false`,
/// `yes`/`no`, `on`/`off` (case-insensitive). Unset/empty → `None`.
pub const PIXI_GIT_LFS_ENV: &str = "PIXI_GIT_LFS";

/// Reads the LFS preference from [`PIXI_GIT_LFS_ENV`].
pub fn lfs_enabled_from_env() -> Option<bool> {
    rattler_git::source::lfs_enabled_from_env(PIXI_GIT_LFS_ENV)
}

/// The [`CheckoutOptions`] pixi uses for git checkouts: submodules are
/// always initialized, and LFS fetching follows [`PIXI_GIT_LFS_ENV`].
pub fn default_checkout_options() -> CheckoutOptions {
    CheckoutOptions {
        update_submodules: true,
        lfs: lfs_enabled_from_env(),
    }
}

/// Bridges a [`rattler_networking::LazyClient`] into the [`LazyClient`]
/// `rattler_git` expects, preserving laziness: the underlying reqwest
/// client is only built if the git code actually issues an HTTP request
/// (the GitHub fast path).
pub fn to_git_client(client: rattler_networking::LazyClient) -> LazyClient {
    LazyClient::new(move || client.client().clone())
}
