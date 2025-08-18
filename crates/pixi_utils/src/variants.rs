use std::collections::BTreeMap;

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
///
/// TODO: In the future we should turn this into a proper type.
pub type VariantConfig = BTreeMap<String, Vec<String>>;
