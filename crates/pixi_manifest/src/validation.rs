use itertools::{Either, Itertools};
use miette::{IntoDiagnostic, LabeledSpan, NamedSource, Report, WrapErr};
use rattler_conda_types::Platform;
use std::{
    collections::HashSet,
    ops::Range,
    path::{Path, PathBuf},
};

use super::pypi::pypi_options::PypiOptions;
use crate::{
    Environment, Feature, FeatureName, KnownPreviewFeature, SystemRequirements, TargetSelector,
    WorkspaceManifest,
};

impl WorkspaceManifest {
    /// Validate the project manifest.
    pub fn validate(&self, source: NamedSource<String>, root_folder: &Path) -> miette::Result<()> {
        // Check if the targets are defined for existing platforms
        for feature in self.features.values() {
            let platforms = feature
                .platforms
                .as_ref()
                .unwrap_or(&self.workspace.platforms);
            for target_sel in feature.targets.user_defined_selectors() {
                match target_sel {
                    TargetSelector::Platform(p) => {
                        if !platforms.as_ref().contains(p) {
                            return Err(create_unsupported_platform_report(
                                source,
                                feature.targets.source_loc(target_sel).unwrap_or_default(),
                                &[p],
                                feature,
                            ));
                        }
                    }
                    TargetSelector::Linux => {
                        if !platforms.as_ref().iter().any(|p| p.is_linux()) {
                            return Err(create_unsupported_platform_report(
                                source,
                                feature.targets.source_loc(target_sel).unwrap_or_default(),
                                &[
                                    &Platform::Linux64,
                                    &Platform::LinuxAarch64,
                                    &Platform::LinuxPpc64le,
                                ],
                                feature,
                            ));
                        }
                    }
                    TargetSelector::MacOs => {
                        if !platforms.as_ref().iter().any(|p| p.is_osx()) {
                            return Err(create_unsupported_platform_report(
                                source,
                                feature.targets.source_loc(target_sel).unwrap_or_default(),
                                &[&Platform::OsxArm64, &Platform::Osx64],
                                feature,
                            ));
                        }
                    }
                    TargetSelector::Win => {
                        if !platforms.as_ref().iter().any(|p| p.is_windows()) {
                            return Err(create_unsupported_platform_report(
                                source,
                                feature.targets.source_loc(target_sel).unwrap_or_default(),
                                &[&Platform::Win64, &Platform::WinArm64],
                                feature,
                            ));
                        }
                    }
                    TargetSelector::Unix => {
                        if !platforms.as_ref().iter().any(|p| p.is_unix()) {
                            return Err(create_unsupported_platform_report(
                                source,
                                feature.targets.source_loc(target_sel).unwrap_or_default(),
                                &[&Platform::Linux64],
                                feature,
                            ));
                        }
                    }
                }
            }
        }

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
        if let Some(preview) = self.workspace.preview.as_ref() {
            let preview = preview.unknown_preview_features();
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
        }

        // Check if the pixi build feature is enabled
        let build_enabled = self
            .workspace
            .preview
            .as_ref()
            .map(|p| p.is_enabled(KnownPreviewFeature::PixiBuild))
            .unwrap_or(false);

        // Error any conda source dependencies are used and is not set
        if !build_enabled {
            let supported_platforms = self.workspace.platforms.as_ref();
            // Check all features for source dependencies
            for feature in self.features.values() {
                if is_using_source_deps(feature, supported_platforms.iter()) {
                    return Err(miette::miette!(
                        help = "enable the `build` preview feature to use source dependencies",
                        "source dependencies are used in the feature '{}', but the `pixi-build` preview feature is not enabled",
                        feature.name
                    ));
                }
            }
        }

        if let Some(build) = &self.build {
            // Check if we have enabled the build feature if we have a build section
            if !build_enabled {
                return Err(miette::miette!(
                    help = "enable the build preview feature to use the build section by setting `preview = [\"pixi-build\"]",
                    "the build section is defined, but the `pixi-build` preview feature is not enabled"
                ));
            }

            // If there is a build section, make sure the build-string is not empty
            if build.build_backend.is_empty() {
                return Err(miette::miette!(
                    help = "the build-backend must contain at least one command. e.g `pixi-build-python`",
                    "the build-backend is empty"
                ));
            }
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
        features
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

        Ok(())
    }
}

/// Check if any feature is making use of conda source dependencies
fn is_using_source_deps<'a>(
    feature: &Feature,
    supported_platforms: impl IntoIterator<Item = &'a Platform>,
) -> bool {
    // List all spec types
    let spec_types = [
        crate::SpecType::Build,
        crate::SpecType::Run,
        crate::SpecType::Host,
    ];
    // Check if any of the spec types have source dependencies
    for platform in supported_platforms {
        for spec in spec_types {
            let deps = feature.dependencies(spec, Some(*platform));
            if let Some(deps) = deps {
                if deps.iter().any(|(_, spec)| spec.is_source()) {
                    return true;
                }
            }
        }
    }

    false
}

// Create an error report for using a platform that is not supported by the
// project.
fn create_unsupported_platform_report(
    source: NamedSource<String>,
    span: Range<usize>,
    platform: &[&Platform],
    feature: &Feature,
) -> Report {
    let platform = platform.iter().map(|p| p.to_string()).join(", ");

    miette::miette!(
        labels = vec![LabeledSpan::at(
            span,
            format!("'{}' is not a supported platform", platform)
        )],
        help = format!(
            "Add any of '{platform}' to the `{}` array of the TOML manifest.",
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
    // TODO: add a test to verify that conflicting system requirements result in
    // an error.
}
