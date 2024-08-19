use std::fmt;

#[derive(Debug, Clone)]
/// Represents a mutable pixi global TOML.
pub(crate) struct ManifestSource(pub(crate) toml_edit::DocumentMut);

impl fmt::Display for ManifestSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
