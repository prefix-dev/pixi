//! Variants trait
//!
use std::collections::BTreeMap;

use miette::IntoDiagnostic;
use rattler_build::{NormalizedKey, recipe::variable::Variable, variant_config::VariantConfig};
use rattler_conda_types::Platform;

use crate::ProjectModel;

/// Return variants for the given project model
pub fn compute_variants<P: ProjectModel>(
    project_model: &P,
    input_variant_configuration: Option<BTreeMap<NormalizedKey, Vec<Variable>>>,
    host_platform: Platform,
) -> miette::Result<Vec<BTreeMap<NormalizedKey, Variable>>> {
    // Create a variant config from the variant configuration in the parameters.
    let variant_config = VariantConfig {
        variants: input_variant_configuration.unwrap_or_default(),
        pin_run_as_build: None,
        zip_keys: None,
    };

    // Determine the variant keys that are used in the recipe.
    let used_variants = project_model.used_variants(Some(host_platform));

    // Determine the combinations of the used variants.
    variant_config
        .combinations(&used_variants, None)
        .into_diagnostic()
}
