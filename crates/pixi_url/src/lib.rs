//! Utilities to download and unpack archives from arbitrary URLs.

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
