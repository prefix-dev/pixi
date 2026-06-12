//! Project behaviour traits.
//!
//! # Key components
//!
//! * [`ProjectModel`] - Core trait for project model interface

use std::collections::HashSet;

use itertools::Itertools;
use pixi_build_types::{self as pbt};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::Version;

use super::{Dependencies, PackageSpec, targets::Targets};

/// A trait that defines the project model interface
pub trait ProjectModel {
    /// The targets type of the project model
    type Targets: Targets;

    /// Return the targets of the project model
    fn targets(&self) -> Option<&Self::Targets>;

    /// Return the dependencies of the project model
    fn dependencies(&self) -> Dependencies<'_, <<Self as ProjectModel>::Targets as Targets>::Spec> {
        self.targets().map(|t| t.dependencies()).unwrap_or_default()
    }

    /// Return the used variants of the project model
    fn used_variants(&self) -> HashSet<NormalizedKey>;

    /// Return the name of the project model
    fn name(&self) -> Option<&String>;

    /// Return the version of the project model
    fn version(&self) -> &Option<Version>;
}

impl ProjectModel for pbt::ProjectModel {
    type Targets = pbt::Targets;

    fn targets(&self) -> Option<&Self::Targets> {
        self.targets.as_ref()
    }

    fn name(&self) -> Option<&String> {
        self.name.as_ref()
    }

    fn version(&self) -> &Option<Version> {
        &self.version
    }

    fn used_variants(&self) -> HashSet<NormalizedKey> {
        // Conditional dependencies are included as a may-use
        // over-approximation: a variant key that only appears under an
        // `if(...)` condition must still produce variant combinations.
        // Spurious keys are harmless because the build hash only
        // incorporates actually used variables.
        let dependencies = self
            .targets()
            .iter()
            .flat_map(|targets| [targets.dependencies(), targets.conditional_dependencies()])
            .collect_vec();

        dependencies
            .iter()
            .flat_map(|deps| {
                deps.build
                    .iter()
                    .chain(deps.host.iter())
                    .chain(deps.run.iter())
            })
            .filter(|(_, spec)| spec.can_be_used_as_variant())
            .map(|(name, _)| name.as_str().into())
            .collect()
    }
}

/// Return a spec of a project model that matches any version
pub fn new_spec<P: ProjectModel>() -> <<P as ProjectModel>::Targets as Targets>::Spec {
    P::Targets::empty_spec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_variants_includes_conditional_dependencies() {
        let model: pbt::ProjectModel = serde_json::from_value(serde_json::json!({
            "name": "example",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "runDependencies": {
                        "boltons": { "binary": { "version": "*" } }
                    }
                },
                "conditional": {
                    "unix": {
                        "hostDependencies": {
                            "openssl": { "binary": { "version": "*" } }
                        }
                    }
                }
            }
        }))
        .unwrap();

        let used_variants = model.used_variants();
        assert!(
            used_variants.contains(&NormalizedKey::from("boltons")),
            "default target dependency names should be reported"
        );
        assert!(
            used_variants.contains(&NormalizedKey::from("openssl")),
            "dependency names that only appear under a condition should be reported"
        );
    }
}
