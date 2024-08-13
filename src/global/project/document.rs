use std::fmt;

pub struct ManifestSource(toml_edit::DocumentMut);

impl fmt::Display for ManifestSource {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
