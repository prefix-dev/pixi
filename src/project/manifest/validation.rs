use crate::project::manifest::{Environment, FeatureName, SystemRequirements};
use crate::{
    consts,
    project::manifest::{Feature, ProjectManifest, TargetSelector},
};
use miette::{IntoDiagnostic, LabeledSpan, NamedSource, Report, WrapErr};
use rattler_conda_types::Platform;
use std::collections::HashSet;
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

impl ProjectManifest {
    /// Validate the project manifest.
    pub fn validate(&self, source: NamedSource, root_folder: &Path) -> miette::Result<()> {
        // Check if the targets are defined for existing platforms
        for feature in self.features.values() {
            let platforms = feature
                .platforms
                .as_ref()
                .unwrap_or(&self.project.platforms);
            for target_sel in feature.targets.user_defined_selectors() {
                match target_sel {
                    TargetSelector::Platform(p) => {
                        if !platforms.as_ref().contains(p) {
                            return Err(create_unsupported_platform_report(
                                source,
                                feature.targets.source_loc(target_sel).unwrap_or_default(),
                                p,
                                feature,
                            ));
                        }
                    }
                }
            }
        }

        // parse the SPDX license expression to make sure that it is a valid expression.
        if let Some(spdx_expr) = &self.project.license {
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

        check_file_existence(&self.project.license_file)?;
        check_file_existence(&self.project.readme)?;

        // Validate the environments defined in the project
        for (_name, env) in self.environments.iter() {
            if let Err(report) = self.validate_environment(env) {
                return Err(report.with_source_code(source));
            }
        }

        Ok(())
    }

    /// Validates that the given environment is valid.
    fn validate_environment(&self, env: &Environment) -> Result<(), Report> {
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

        // Check if there are conflicts in system requirements between features
        if let Err(e) = features
            .iter()
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

        Ok(())
    }
}

// Create an error report for using a platform that is not supported by the project.
fn create_unsupported_platform_report(
    source: NamedSource,
    span: Range<usize>,
    platform: &Platform,
    feature: &Feature,
) -> Report {
    miette::miette!(
        labels = vec![LabeledSpan::at(
            span,
            format!("'{}' is not a supported platform", platform)
        )],
        help = format!(
            "Add '{platform}' to the `{}` array of the {} manifest.",
            consts::PROJECT_MANIFEST,
            if feature.platforms.is_some() {
                format!(
                    "feature.{}.platforms",
                    feature
                        .name
                        .name()
                        .expect("default feature never defines custom platforms")
                )
            } else {
                String::from("project.platforms")
            }
        ),
        "targeting a platform that this project does not support"
    )
    .with_source_code(source)
}

#[cfg(test)]
mod tests {
    // TODO: add a test to verify that conflicting system requirements result in an error.
}
