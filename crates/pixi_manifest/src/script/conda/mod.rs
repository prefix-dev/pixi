//! The `conda-script` metadata block: conda channels, dependencies and an
//! entrypoint embedded in a comment block of any code file.
//!
//! A block opens with a line ending in `/// conda-script`, whose leading
//! comment characters become the prefix every following line must carry, and
//! closes with the prefix followed by `/// end-conda-script`. The content is
//! TOML 1.1, which allows multiline inline tables.

mod document;
mod entrypoint;
mod envelope;
mod error;
mod manifest;
mod metadata;

pub use document::CondaScriptManifestDocument;
pub use entrypoint::{Entrypoint, EntrypointSelector};
pub use error::{CondaScriptError, EnvelopeError, MetadataError};
pub use manifest::CondaScriptManifest;
pub use metadata::{CondaScriptMetadata, PixiTool};
