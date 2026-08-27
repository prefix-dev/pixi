//! Manifests embedded in script files.
//!
//! A single code file can carry its manifest in a comment block: Python
//! files use the standardized PEP 723 `script` block, while files of any
//! language can use the `conda-script` block proposed in
//! <https://github.com/prefix-dev/pixi/issues/3751>. The `block` module
//! holds the machinery both kinds share: comment-prefix stripping, source
//! maps for diagnostics, and serializing edits back into the block.

mod block;
pub mod conda;
mod pep723;

pub use pep723::{
    ScriptManifest, ScriptManifestDocument, ScriptManifestError, ScriptMetadataError,
    ScriptWorkspaceConfig,
};
