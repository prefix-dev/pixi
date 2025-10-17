//! A passthrough build backend for testing purposes.
//!
//! This backend simply passes along the information from the project model to
//! the `conda/outputs` API without any modifications. It's useful for testing
//! and debugging purposes, as it does not perform any actual building or
//! processing of the project model.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use ordermap::OrderMap;
use pixi_build_frontend::{
    BackendOutputStream,
    error::BackendError,
    in_memory::{InMemoryBackend, InMemoryBackendInstantiator},
    json_rpc::CommunicationError,
};
use pixi_build_types::{
    BackendCapabilities, BinaryPackageSpecV1, NamedSpecV1, PackageSpecV1, ProjectModelV1,
    SourcePackageName, TargetSelectorV1, TargetV1, TargetsV1, VersionedProjectModel,
    procedures::{
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputMetadata, CondaOutputsParams,
            CondaOutputsResult,
        },
        initialize::InitializeParams,
    },
};
use rattler_conda_types::{PackageName, Platform, Version, VersionSpec, package::IndexJson};
use serde::Deserialize;

const BACKEND_NAME: &str = "passthrough";

/// An in-memory build backend that simply passes along the information from the
/// project model to the `conda/outputs` API without any modifications. This
/// backend is useful for testing and debugging purposes, as it does not perform
/// any actual building or processing of the project model.
pub struct PassthroughBackend {
    project_model: ProjectModelV1,
    config: PassthroughBackendConfig,
    source_dir: PathBuf,
    index_json: Option<IndexJson>,
}

impl PassthroughBackend {
    /// Returns an object that can be used to instantiate a
    /// [`PassthroughBackend`].
    pub fn instantiator() -> impl InMemoryBackendInstantiator<Backend = Self> {
        PassthroughBackendInstantiator
    }
}

impl InMemoryBackend for PassthroughBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            provides_conda_outputs: Some(true),
            provides_conda_build_v1: Some(true),
            ..BackendCapabilities::default()
        }
    }

    fn identifier(&self) -> &str {
        BACKEND_NAME
    }

    fn conda_outputs(
        &self,
        params: CondaOutputsParams,
    ) -> Result<CondaOutputsResult, CommunicationError> {
        // Generate outputs for all variant combinations
        let outputs = generate_variant_outputs(&self.project_model, &self.index_json, &params);

        Ok(CondaOutputsResult {
            outputs,
            input_globs: Default::default(),
        })
    }

    fn conda_build_v1(
        &self,
        params: CondaBuildV1Params,
        _output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaBuildV1Result, CommunicationError> {
        let (Some(index_json), Some(package)) = (&self.index_json, &self.config.package) else {
            return Err(
                BackendError::new("no 'package' configured for passthrough backend").into(),
            );
        };
        let absolute_path = self.source_dir.join(package);
        let output_file = params
            .output_directory
            .unwrap_or(params.work_directory)
            .join(package);
        fs_err::copy(absolute_path, &output_file).unwrap();

        Ok(CondaBuildV1Result {
            output_file,
            input_globs: self.config.build_globs.clone().unwrap_or_default(),
            name: index_json.name.as_normalized().to_owned(),
            version: index_json.version.clone(),
            build: index_json.build.clone(),
            subdir: index_json
                .subdir
                .as_ref()
                .expect("missing subdir in index.json")
                .parse()
                .expect("invalid subdir in index.json"),
        })
    }
}

/// Generates all variant outputs for a package based on the variant configuration.
///
/// If any dependency has a "*" version requirement and there's a variant configuration
/// for that package, multiple outputs will be generated - one for each variant combination.
fn generate_variant_outputs(
    project_model: &ProjectModelV1,
    index_json: &Option<IndexJson>,
    params: &CondaOutputsParams,
) -> Vec<CondaOutput> {
    // Check if we have variant configurations and dependencies with "*"
    let variant_keys = find_variant_keys(project_model, params);

    if variant_keys.is_empty() {
        // No variants needed, return single output
        return vec![create_output(
            project_model,
            index_json,
            params,
            &BTreeMap::new(),
        )];
    }

    // Get variant values for each key from the configuration
    let variant_values: Vec<(String, Vec<String>)> = variant_keys
        .into_iter()
        .filter_map(|key| {
            params
                .variant_configuration
                .as_ref()
                .and_then(|config| config.get(&key))
                .map(|values| (key, values.clone()))
        })
        .collect();

    if variant_values.is_empty() {
        // No variant values found, return single output
        return vec![create_output(
            project_model,
            index_json,
            params,
            &BTreeMap::new(),
        )];
    }

    // Generate all combinations of variant values
    let combinations = generate_variant_combinations(&variant_values);

    // Create an output for each variant combination
    combinations
        .iter()
        .map(|variant| create_output(project_model, index_json, params, variant))
        .collect()
}

/// Finds all dependency names that have "*" requirements and have variant configurations.
fn find_variant_keys(project_model: &ProjectModelV1, params: &CondaOutputsParams) -> Vec<String> {
    let Some(targets) = &project_model.targets else {
        return Vec::new();
    };

    let Some(variant_config) = &params.variant_configuration else {
        return Vec::new();
    };

    let mut variant_keys = BTreeSet::new();

    // Helper to check dependencies in a target
    let mut check_deps = |deps: Option<&OrderMap<SourcePackageName, PackageSpecV1>>| {
        if let Some(deps) = deps {
            for (name, spec) in deps {
                // Check if this dependency has a "*" requirement
                if is_star_requirement(spec) {
                    let name_str = name.as_str();
                    // Check if there's a variant configuration for this package
                    if variant_config.contains_key(name_str) {
                        variant_keys.insert(name_str.to_string());
                    }
                }
            }
        }
    };

    // Check default target
    if let Some(default_target) = &targets.default_target {
        check_deps(default_target.build_dependencies.as_ref());
        check_deps(default_target.host_dependencies.as_ref());
        check_deps(default_target.run_dependencies.as_ref());
    }

    // Check platform-specific targets
    if let Some(targets_map) = &targets.targets {
        for (selector, target) in targets_map {
            if matches_target_selector(selector, params.host_platform) {
                check_deps(target.build_dependencies.as_ref());
                check_deps(target.host_dependencies.as_ref());
                check_deps(target.run_dependencies.as_ref());
            }
        }
    }

    variant_keys.into_iter().collect()
}

/// Checks if a package spec has a "*" version requirement.
fn is_star_requirement(spec: &PackageSpecV1) -> bool {
    let PackageSpecV1::Binary(boxed) = spec else {
        return false;
    };

    match boxed.as_ref() {
        BinaryPackageSpecV1 {
            version,
            build: None,
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
            url: None,
            license: None,
        } => version
            .as_ref()
            .is_none_or(|v| matches!(v, VersionSpec::Any)),
        _ => false,
    }
}

/// Generates all combinations of variant values using a Cartesian product.
///
/// For example, if we have:
/// - python: ["3.10", "3.11"]
/// - numpy: ["1.0", "2.0"]
///
/// This will generate 4 combinations:
/// - {python: "3.10", numpy: "1.0"}
/// - {python: "3.10", numpy: "2.0"}
/// - {python: "3.11", numpy: "1.0"}
/// - {python: "3.11", numpy: "2.0"}
fn generate_variant_combinations(
    variant_values: &[(String, Vec<String>)],
) -> Vec<BTreeMap<String, String>> {
    use itertools::Itertools;

    if variant_values.is_empty() {
        return vec![BTreeMap::new()];
    }

    // Extract just the values for the cartesian product
    let value_lists: Vec<_> = variant_values
        .iter()
        .map(|(_, values)| values.as_slice())
        .collect();

    // Generate all combinations using multi_cartesian_product
    value_lists
        .into_iter()
        .multi_cartesian_product()
        .map(|combination| {
            // Zip the keys with the values from this combination
            variant_values
                .iter()
                .map(|(key, _)| key)
                .zip(combination)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .collect()
}

/// Creates a single output with the given variant configuration.
fn create_output(
    project_model: &ProjectModelV1,
    index_json: &Option<IndexJson>,
    params: &CondaOutputsParams,
    variant: &BTreeMap<String, String>,
) -> CondaOutput {
    CondaOutput {
        metadata: CondaOutputMetadata {
            name: project_model
                .name
                .as_ref()
                .map(|name| PackageName::try_from(name.as_str()).unwrap())
                .unwrap_or_else(|| {
                    index_json
                        .as_ref()
                        .map(|j| j.name.clone())
                        .unwrap_or_else(|| PackageName::try_from("pixi-package_name").unwrap())
                }),
            version: project_model
                .version
                .as_ref()
                .or_else(|| index_json.as_ref().map(|j| j.version.version()))
                .cloned()
                .unwrap_or_else(|| Version::major(0))
                .into(),
            build: index_json
                .as_ref()
                .map(|j| j.build.clone())
                .unwrap_or_default(),
            build_number: index_json
                .as_ref()
                .map(|j| j.build_number)
                .unwrap_or_default(),
            subdir: index_json
                .as_ref()
                .and_then(|j| j.subdir.as_deref())
                .map(|subdir| subdir.parse().unwrap())
                .unwrap_or(Platform::NoArch),
            license: project_model.license.clone(),
            license_family: None,
            noarch: index_json.as_ref().map(|j| j.noarch).unwrap_or_default(),
            purls: None,
            python_site_packages_path: None,
            variant: variant.clone(),
        },
        build_dependencies: Some(extract_dependencies(
            &project_model.targets,
            |t| t.build_dependencies.as_ref(),
            params.host_platform,
            variant,
        )),
        host_dependencies: Some(extract_dependencies(
            &project_model.targets,
            |t| t.host_dependencies.as_ref(),
            params.host_platform,
            variant,
        )),
        run_dependencies: extract_dependencies(
            &project_model.targets,
            |t| t.run_dependencies.as_ref(),
            params.host_platform,
            variant,
        ),
        ignore_run_exports: Default::default(),
        run_exports: Default::default(),
        input_globs: None,
    }
}

fn extract_dependencies<F: Fn(&TargetV1) -> Option<&OrderMap<SourcePackageName, PackageSpecV1>>>(
    targets: &Option<TargetsV1>,
    extract: F,
    platform: Platform,
    variant: &BTreeMap<String, String>,
) -> CondaOutputDependencies {
    let depends = targets
        .iter()
        .flat_map(|targets| {
            targets
                .default_target
                .iter()
                .chain(
                    targets
                        .targets
                        .iter()
                        .flatten()
                        .flat_map(|(selector, target)| {
                            matches_target_selector(selector, platform).then_some(target)
                        }),
                )
                .flat_map(|target| extract(target).into_iter().flat_map(OrderMap::iter))
                .map(|(name, spec)| {
                    // If this is a star dependency and we have a variant for it, replace the spec
                    let resolved_spec = if is_star_requirement(spec) {
                        if let Some(variant_value) = variant.get(name.as_str()) {
                            // Replace with a version spec using the variant value
                            PackageSpecV1::Binary(Box::new(BinaryPackageSpecV1 {
                                version: Some(
                                    rattler_conda_types::VersionSpec::from_str(
                                        variant_value,
                                        rattler_conda_types::ParseStrictness::Lenient,
                                    )
                                    .unwrap(),
                                ),
                                ..Default::default()
                            }))
                        } else {
                            spec.clone()
                        }
                    } else {
                        spec.clone()
                    };

                    NamedSpecV1 {
                        name: name.clone(),
                        spec: resolved_spec,
                    }
                })
        })
        .collect();

    CondaOutputDependencies {
        depends,
        constraints: Vec::new(),
    }
}

/// Returns true if the given [`TargetSelectorV1`] matches the specified
/// `platform`.
fn matches_target_selector(selector: &TargetSelectorV1, platform: Platform) -> bool {
    match selector {
        TargetSelectorV1::Unix => platform.is_unix(),
        TargetSelectorV1::Linux => platform.is_linux(),
        TargetSelectorV1::Win => platform.is_windows(),
        TargetSelectorV1::MacOs => platform.is_osx(),
        TargetSelectorV1::Platform(target_platform) => target_platform == platform.as_str(),
    }
}

/// An implementation of the [`InMemoryBackendInstantiator`] that creates a
/// [`PassthroughBackend`].
pub struct PassthroughBackendInstantiator;

impl InMemoryBackendInstantiator for PassthroughBackendInstantiator {
    type Backend = PassthroughBackend;

    fn initialize(&self, params: InitializeParams) -> Result<Self::Backend, CommunicationError> {
        let project_model = match params.project_model {
            Some(VersionedProjectModel::V1(project_model)) => project_model,
            _ => {
                return Err(CommunicationError::BackendError(BackendError::new(
                    "Passthrough backend only supports project model v1",
                )));
            }
        };

        let config = match params.configuration {
            Some(config) => serde_json::from_value(config).expect("Failed to parse configuration"),
            None => PassthroughBackendConfig::default(),
        };

        // Read the package file if it is specified
        let source_dir = params.source_dir.expect("Missing source directory");
        let index_json = match &config.package {
            Some(path) => {
                let path = source_dir.join(path);
                match rattler_package_streaming::seek::read_package_file(&path) {
                    Err(err) => {
                        return Err(BackendError::new(format!(
                            "failed to read '{}' file: {}",
                            path.display(),
                            err
                        ))
                        .into());
                    }
                    Ok(index_json) => Some(index_json),
                }
            }
            None => None,
        };

        Ok(PassthroughBackend {
            project_model,
            config,
            source_dir,
            index_json,
        })
    }

    fn identifier(&self) -> &str {
        BACKEND_NAME
    }
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PassthroughBackendConfig {
    /// The path to a pre-build conda package.
    pub package: Option<PathBuf>,

    /// Build globs
    pub build_globs: Option<BTreeSet<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixi_build_types::{BinaryPackageSpecV1, PackageSpecV1};
    use rattler_conda_types::{ParseStrictness, VersionSpec};

    #[test]
    fn test_is_star_requirement_with_star() {
        let spec = PackageSpecV1::Binary(Box::new(BinaryPackageSpecV1 {
            version: Some(VersionSpec::from_str("*", ParseStrictness::Lenient).unwrap()),
            ..Default::default()
        }));

        assert!(is_star_requirement(&spec));
    }

    #[test]
    fn test_is_star_requirement_with_version() {
        let spec = PackageSpecV1::Binary(Box::new(BinaryPackageSpecV1 {
            version: Some(VersionSpec::from_str(">=1.0", ParseStrictness::Lenient).unwrap()),
            ..Default::default()
        }));

        assert!(!is_star_requirement(&spec));
    }

    #[test]
    fn test_is_star_requirement_with_no_version() {
        let spec = PackageSpecV1::Binary(Box::default());

        assert!(is_star_requirement(&spec));
    }

    #[test]
    fn test_generate_variant_combinations_empty() {
        let variants = generate_variant_combinations(&[]);
        assert_eq!(variants.len(), 1);
        assert!(variants[0].is_empty());
    }

    #[test]
    fn test_generate_variant_combinations_single() {
        let variants = generate_variant_combinations(&[(
            "python".to_string(),
            vec!["3.10".to_string(), "3.11".to_string()],
        )]);

        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].get("python").unwrap(), "3.10");
        assert_eq!(variants[1].get("python").unwrap(), "3.11");
    }

    #[test]
    fn test_generate_variant_combinations_multiple() {
        let variants = generate_variant_combinations(&[
            (
                "python".to_string(),
                vec!["3.10".to_string(), "3.11".to_string()],
            ),
            (
                "numpy".to_string(),
                vec!["1.0".to_string(), "2.0".to_string()],
            ),
        ]);

        assert_eq!(variants.len(), 4);

        // Verify all combinations exist
        let expected = vec![
            ("3.10", "1.0"),
            ("3.10", "2.0"),
            ("3.11", "1.0"),
            ("3.11", "2.0"),
        ];

        for (expected_python, expected_numpy) in expected {
            assert!(
                variants
                    .iter()
                    .any(|v| v.get("python").unwrap() == expected_python
                        && v.get("numpy").unwrap() == expected_numpy),
                "Expected combination ({}, {}) not found",
                expected_python,
                expected_numpy
            );
        }
    }

    #[test]
    fn test_generate_variant_combinations_three_dimensions() {
        let variants = generate_variant_combinations(&[
            (
                "python".to_string(),
                vec!["3.10".to_string(), "3.11".to_string()],
            ),
            (
                "numpy".to_string(),
                vec!["1.0".to_string(), "2.0".to_string()],
            ),
            (
                "os".to_string(),
                vec!["linux".to_string(), "windows".to_string()],
            ),
        ]);

        // Should generate 2 * 2 * 2 = 8 combinations
        assert_eq!(variants.len(), 8);

        // Verify all keys are present in each variant
        for variant in &variants {
            assert!(variant.contains_key("python"));
            assert!(variant.contains_key("numpy"));
            assert!(variant.contains_key("os"));
        }
    }

    #[test]
    fn test_generate_variant_combinations_single_value() {
        let variants = generate_variant_combinations(&[
            ("python".to_string(), vec!["3.10".to_string()]),
            ("numpy".to_string(), vec!["1.0".to_string()]),
        ]);

        // Should generate only 1 combination
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].get("python").unwrap(), "3.10");
        assert_eq!(variants[0].get("numpy").unwrap(), "1.0");
    }
}
