use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use itertools::{Either, Itertools};
use miette::{IntoDiagnostic, LabeledSpan, NamedSource, Report, WrapErr};

use super::pypi::pypi_options::PypiOptions;
use crate::{
    pypi::pypi_options::NoBuild, Environment, Feature, FeatureName, SystemRequirements,
    WorkspaceManifest,
};

impl WorkspaceManifest {
    /// Validate the project manifest.
    pub fn validate(&self, source: NamedSource<String>, root_folder: &Path) -> miette::Result<()> {
        // Check if all features are used in environments, warn if not.
        let mut features_used = HashSet::new();
        for env in self.environments.iter() {
            for feature in env.features.iter() {
                features_used.insert(feature);
            }
        }
        for (name, _feature) in self.features.iter() {
            if name != &FeatureName::Default && !features_used.contains(&name.to_string()) {
                tracing::warn!(
                    "The feature '{}' is defined but not used in any environment",
                    name,
                );
            }
        }

        // parse the SPDX license expression to make sure that it is a valid expression.
        if let Some(spdx_expr) = &self.workspace.license {
            spdx::Expression::parse(spdx_expr)
                .into_diagnostic()
                .with_context(|| {
                    format!(
                        "failed to parse the SPDX license expression '{}'",
                        spdx_expr
                    )
                })?;
        }

        let check_file_existence = |x: &Option<PathBuf>| {
            if let Some(path) = x {
                let full_path = root_folder.join(path);
                if !full_path.exists() {
                    return Err(miette::miette!(
                        "the file '{}' does not exist",
                        full_path.display()
                    ));
                }
            }
            Ok(())
        };

        check_file_existence(&self.workspace.license_file)?;
        check_file_existence(&self.workspace.readme)?;

        // Validate the environments defined in the project
        for env in self.environments.iter() {
            if let Err(report) = self.validate_environment(env, self.default_feature()) {
                return Err(report.with_source_code(source));
            }
        }

        // Warn on any unknown preview features
        let preview = self.workspace.preview.unknown_preview_features();
        if !preview.is_empty() {
            let are = if preview.len() > 1 { "are" } else { "is" };
            let s = if preview.len() > 1 { "s" } else { "" };
            let preview_array = if preview.len() == 1 {
                format!("{:?}", preview)
            } else {
                format!("[{:?}]", preview.iter().format(", "))
            };
            tracing::warn!(
                "The preview feature{s}: {preview_array} {are} defined in the manifest but un-used pixi");
        }

        Ok(())
    }

    /// Validates that the given environment is valid.
    fn validate_environment(
        &self,
        env: &Environment,
        default_feature: &Feature,
    ) -> Result<(), Report> {
        let mut features_seen = HashSet::new();
        let mut features = Vec::with_capacity(env.features.len());
        for feature in env.features.iter() {
            // Make sure that the environment does not have any duplicate features.
            if !features_seen.insert(feature) {
                return Err(miette::miette!(
                    labels = vec![LabeledSpan::at(
                        env.features_source_loc.clone().unwrap_or_default(),
                        format!("the feature '{}' was defined more than once.", feature)
                    )],
                    help =
                        "since the order of the features matters a duplicate feature is ambiguous",
                    "the feature '{}' is defined multiple times in the environment '{}'",
                    feature,
                    env.name.as_str()
                ));
            }

            // Make sure that every feature actually exists.
            match self.features.get(&FeatureName::Named(feature.clone())) {
                Some(feature) => features.push(feature),
                None => {
                    return Err(miette::miette!(
                        labels = vec![LabeledSpan::at(
                            env.features_source_loc.clone().unwrap_or_default(),
                            format!("unknown feature '{}'", feature)
                        )],
                        help = "add the feature to the project manifest",
                        "the feature '{}' is not defined in the project manifest",
                        feature
                    ));
                }
            }
        }

        // Choose whether to include the default
        let default = if env.no_default_feature {
            Either::Left(std::iter::empty())
        } else {
            Either::Right(std::iter::once(&default_feature))
        };

        // Check if there are conflicts in system requirements between features
        if let Err(e) = features
            .iter()
            .chain(default.clone())
            .map(|feature| &feature.system_requirements)
            .try_fold(SystemRequirements::default(), |acc, req| acc.union(req))
        {
            return Err(miette::miette!(
                labels = vec![LabeledSpan::at(
                    env.features_source_loc.clone().unwrap_or_default(),
                    "while resolving system requirements of features defined here"
                )],
                "{e}",
            ));
        }

        // Check if there are no conflicts in pypi options between features
        let opts = features
            .iter()
            .chain(default)
            .filter_map(|feature| {
                if feature.pypi_options().is_none() {
                    // Use the project default features
                    self.workspace.pypi_options.as_ref()
                } else {
                    feature.pypi_options()
                }
            })
            .try_fold(PypiOptions::default(), |acc, opts| acc.union(opts))
            .into_diagnostic()?;

        // If no-build is set, check if the package names are pep508 compliant
        if let Some(NoBuild::Packages(packages)) = opts.no_build {
            let packages = packages
                .iter()
                .map(|p| pep508_rs::PackageName::new(p.clone()))
                .collect::<Result<Vec<_>, _>>();
            if let Err(e) = packages {
                return Err(miette::miette!(
                    labels = vec![LabeledSpan::at(
                        env.features_source_loc.clone().unwrap_or_default(),
                        "while resolving no-build packages array"
                    )],
                    "{e}",
                ));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    // TODO: add a test to verify that conflicting system requirements result in
    // an error.
}
