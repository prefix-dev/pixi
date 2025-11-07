//! Utilities to download and unpack archives from arbitrary URLs.
//!
//! The crate mirrors the structure of `pixi_git`: a tiny public surface in
//! `lib.rs`, a resolver that deduplicates work across a single process, and a
//! source object that owns the heavy lifting (locking, downloading, hashing,
//! extracting).  This separation keeps the async download/extraction logic
//! reusable for other crates, while the resolver provides the higher-level
//! “fetch once, then reuse the precise hash everywhere” behavior the command
//! dispatcher expects.  Archive-specific work lives in `extract.rs`, and
//! progress reporting is abstracted behind `progress.rs` so consumers can plug
//! in their own UI without pulling in dispatcher-specific types.

mod error;
pub mod extract;
pub mod progress;
pub mod resolver;
mod source;
mod util;

pub use error::{ExtractError, UrlError};
pub use progress::{NoProgressHandler, ProgressHandler};
pub use resolver::UrlResolver;
pub use source::{Fetch, UrlSource};
