//! Source-spec resolution and checkout Keys for the pixi compute
//! engine.
//!
//! Owns everything needed to turn a source spec (path / git / url) into
//! bytes on disk, plus the engine-side accessors for the resolvers and
//! cache markers it depends on. See [`SourceCheckout`] for the shared
//! result type, [`SourceCheckoutExt`] for the per-spec checkout
//! methods on [`pixi_compute_engine::ComputeCtx`], and the [`git`] and
//! [`url`] modules for the per-source Keys.

mod checkout;
mod data;
mod ext;
pub mod git;
mod path;
pub mod url;

pub use checkout::{InvalidPathError, SourceCheckout, SourceCheckoutError};
pub use data::{HasGitResolver, HasUrlResolver};
pub use ext::SourceCheckoutExt;
pub use git::{
    CheckoutGit, GitCheckoutReporter, GitCheckoutSemaphore, GitDir, GitSourceCheckoutExt,
    HasGitCheckoutReporter, HasGitCheckoutSemaphore,
};
pub use path::{RootDir, RootDirExt};
pub use url::{
    CheckoutUrl, HasUrlCheckoutReporter, HasUrlCheckoutSemaphore, UrlCheckout, UrlCheckoutReporter,
    UrlCheckoutSemaphore, UrlDir, UrlSourceCheckoutExt,
};
