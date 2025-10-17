use std::{collections::BTreeMap, path::PathBuf};

/// An alias for variant configuration. This maps a variant name to a list of
/// options.
///
/// E.g.
///
/// ```yaml
/// python:
///     - 3.8
///     - 3.9
/// numpy:
///     - 1.18
/// ```
#[derive(Debug, Clone)]
pub struct VariantConfig {
    /// Inline variant configuration
    pub variants: BTreeMap<String, Vec<String>>,

    /// Absolute paths to external variant files.
    pub variant_files: Vec<PathBuf>,
}
