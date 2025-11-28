use std::{collections::BTreeMap, path::PathBuf};

pub use pixi_variant::VariantValue;

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
    pub variant_configuration: BTreeMap<String, Vec<VariantValue>>,

    /// Absolute paths to external variant files.
    pub variant_files: Vec<PathBuf>,
}
