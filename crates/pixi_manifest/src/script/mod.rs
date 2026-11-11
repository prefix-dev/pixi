//! Manifests embedded in script files.
//!
//! A single code file can carry its manifest in a comment block: Python
//! files use the standardized PEP 723 `script` block, while files of any
//! language can use the `conda-script` block. The `block` module
//! holds the machinery both kinds share: comment-prefix stripping, source
//! maps for diagnostics, and serializing edits back into the block. The
//! `tool_pixi` module holds the subset of `tool.pixi` both kinds accept.

mod block;
pub mod conda;
mod pep723;
mod tool_pixi;

pub use pep723::{
    ScriptManifest, ScriptManifestDocument, ScriptManifestError, ScriptMetadataError,
    ScriptWorkspaceConfig,
};
