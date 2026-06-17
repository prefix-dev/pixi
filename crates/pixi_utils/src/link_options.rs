//! Configured allow/disallow preferences for installation link methods.

/// Configured allow/disallow preferences for installation link methods
/// (symbolic links, hard links, and ref links / copy-on-write).
///
/// Mirrors the fields on `rattler::install::LinkOptions` but lives in a
/// crate shared by both the conda and PyPI installation pipelines so it can
/// be stored once in the compute engine's data store, keyed by `TypeId`.
#[derive(Copy, Clone, Debug, Default)]
pub struct AllowLinkOptions {
    pub allow_symbolic_links: Option<bool>,
    pub allow_hard_links: Option<bool>,
    pub allow_ref_links: Option<bool>,
}
