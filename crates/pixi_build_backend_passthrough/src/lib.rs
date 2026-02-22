//! A passthrough build backend for testing purposes.
//!
//! This backend simply passes along the information from the project model to
//! the `conda/outputs` API without any modifications. It's useful for testing
//! and debugging purposes, as it does not perform any actual building or
//! processing of the project model.

use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use ordermap::OrderMap;
use pixi_build_frontend::{
    BackendOutputStream,
    error::BackendError,
    in_memory::{InMemoryBackend, InMemoryBackendInstantiator},
    json_rpc::CommunicationError,
};
use pixi_build_types::{
    BackendCapabilities, BinaryPackageSpec, ConstraintSpec, NamedSpec, PackageSpec, ProjectModel,
    SourcePackageName, Target, TargetSelector, Targets, VariantValue,
    procedures::{
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputMetadata, CondaOutputsParams,
            CondaOutputsResult,
        },
        initialize::InitializeParams,
    },
};
use rattler_conda_types::{
    PackageName, Platform, Version, VersionSpec,
    package::{IndexJson, PathType, PathsEntry, PathsJson, RunExportsJson},
};
use serde::Deserialize;

const BACKEND_NAME: &str = "passthrough";

/// Events that can be emitted by the observable backend during method calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendEvent {
    /// Emitted when `conda_build_v1` is called
    CondaBuildV1Called,
    /// Emitted when `conda_outputs` is called
    CondaOutputsCalled,
}

/// An in-memory build backend that simply passes along the information from the
/// project model to the `conda/outputs` API without any modifications. This
/// backend is useful for testing and debugging purposes, as it does not perform
/// any actual building or processing of the project model.
pub struct PassthroughBackend {
    project_model: ProjectModel,
    config: PassthroughBackendConfig,
    source_dir: PathBuf,
    index_json: IndexJson,
    /// Run exports configuration for simulating package run_exports.
    /// Maps package names to their run_exports definitions.
    run_exports: BTreeMap<String, RunExportsJson>,
    /// Run exports read from the package file (when config.package is specified).
    package_run_exports: Option<RunExportsJson>,
}

impl PassthroughBackend {
    /// Returns an object that can be used to instantiate a
    /// [`PassthroughBackend`].
    pub fn instantiator() -> PassthroughBackendInstantiator {
        PassthroughBackendInstantiator::default()
    }
}

impl InMemoryBackend for PassthroughBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            provides_conda_outputs: Some(true),
            provides_conda_build_v1: Some(true),
        }
    }

    fn identifier(&self) -> &str {
        BACKEND_NAME
    }

    fn conda_outputs(
        &self,
        params: CondaOutputsParams,
        _output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaOutputsResult, Box<CommunicationError>> {
        // Generate outputs for all variant combinations
        let outputs = generate_variant_outputs(
            &self.project_model,
            &self.index_json,
            &params,
            &self.run_exports,
            self.package_run_exports.as_ref(),
        );

        Ok(CondaOutputsResult {
            outputs,
            input_globs: Default::default(),
        })
    }

    fn conda_build_v1(
        &self,
        params: CondaBuildV1Params,
        _output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaBuildV1Result, Box<CommunicationError>> {
        // Compute the variant-aware build string (must match what conda_outputs returns)
        let variant: BTreeMap<String, VariantValue> = params
            .output
            .variant
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Check if there are real variants (not just target_platform)
        let has_real_variants = variant.keys().any(|k| k != "target_platform");
        let build_string =
            compute_build_string(&self.index_json.build, &variant, has_real_variants);

        let output_dir = params
            .output_directory
            .unwrap_or(params.work_directory.clone());

        // Determine the subdir - use the one from index_json if present, otherwise default to NoArch
        let subdir = self
            .index_json
            .subdir
            .as_ref()
            .map(|s| s.parse().expect("invalid subdir in index.json"))
            .unwrap_or(Platform::NoArch);

        let output_file = match &self.config.package {
            Some(package) => {
                let absolute_path = self.source_dir.join(package);
                let output_path = output_dir.join(package);
                fs_err::copy(absolute_path, &output_path).unwrap();
                output_path
            }
            None => {
                let file_name = format!(
                    "{}-{}-{}.conda",
                    self.index_json.name.as_normalized(),
                    self.index_json.version,
                    &build_string
                );
                let output_path = output_dir.join(&file_name);

                // Create a modified index_json with augmented fields from project_model
                // This must match what conda_outputs returns in CondaOutputMetadata
                let mut modified_index_json = self.index_json.clone();
                modified_index_json.build = build_string.clone();
                modified_index_json.subdir = Some(subdir.to_string());
                if let Some(name) = &self.project_model.name {
                    modified_index_json.name =
                        PackageName::try_from(name.as_str()).expect("invalid package name");
                }
                if let Some(version) = &self.project_model.version {
                    modified_index_json.version = version.clone().into();
                }
                modified_index_json.license = self.project_model.license.clone();

                create_conda_package_on_the_fly(&modified_index_json, &output_path).map_err(
                    |err| {
                        Box::new(
                            BackendError::new(format!("failed to create conda package: {}", err))
                                .into(),
                        )
                    },
                )?;
                output_path
            }
        };

        Ok(CondaBuildV1Result {
            output_file,
            input_globs: self.config.build_globs.clone().unwrap_or_default(),
            name: self.index_json.name.as_normalized().to_owned(),
            version: self.index_json.version.clone(),
            build: build_string,
            subdir,
        })
    }
}

/// Creates a conda package on the fly with the given IndexJson metadata.
fn create_conda_package_on_the_fly(
    index_json: &IndexJson,
    output_path: &Path,
) -> Result<(), std::io::Error> {
    use rattler_conda_types::compression_level::CompressionLevel;

    // Create a temporary directory to stage the package contents
    let temp_dir = tempfile::tempdir()?;
    let info_dir = temp_dir.path().join("info");
    fs_err::create_dir_all(&info_dir)?;

    // Write index.json
    let index_json_content = serde_json::to_string_pretty(index_json)?;
    let index_json_path = info_dir.join("index.json");
    fs_err::write(&index_json_path, &index_json_content)?;

    // Create paths.json with the index.json entry
    let index_json_bytes = index_json_content.as_bytes();
    let index_json_sha256 =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(index_json_bytes);

    let paths_json = PathsJson {
        paths: vec![PathsEntry {
            relative_path: PathBuf::from("info/index.json"),
            no_link: false,
            path_type: PathType::HardLink,
            prefix_placeholder: None,
            sha256: Some(index_json_sha256),
            size_in_bytes: Some(index_json_bytes.len() as u64),
        }],
        paths_version: 1,
    };

    let paths_json_content = serde_json::to_string_pretty(&paths_json)?;
    let paths_json_path = info_dir.join("paths.json");
    fs_err::write(&paths_json_path, &paths_json_content)?;

    // Collect paths to include in the package
    let paths = vec![info_dir.join("index.json"), info_dir.join("paths.json")];

    // Create the output file
    let output_file = fs_err::File::create(output_path)?;

    // Determine the package name stem (without extension)
    let out_name = format!(
        "{}-{}-{}",
        index_json.name.as_normalized(),
        index_json.version,
        index_json.build
    );

    // Write the conda package
    rattler_package_streaming::write::write_conda_package(
        output_file,
        temp_dir.path(),
        &paths,
        CompressionLevel::Default,
        None, // Use default thread count
        &out_name,
        None, // No specific timestamp
        None, // No progress bar
    )?;

    Ok(())
}

/// Generates all variant outputs for a package based on the variant
/// configuration.
///
/// If any dependency has a "*" version requirement and there's a variant
/// configuration for that package, multiple outputs will be generated - one for
/// each variant combination.
fn generate_variant_outputs(
    project_model: &ProjectModel,
    index_json: &IndexJson,
    params: &CondaOutputsParams,
    run_exports: &BTreeMap<String, RunExportsJson>,
    package_run_exports: Option<&RunExportsJson>,
) -> Vec<CondaOutput> {
    // Check if we have variant configurations and dependencies with "*"
    let variant_keys = find_variant_keys(project_model, params);

    if variant_keys.is_empty() {
        // No variants needed, return single output
        return vec![create_output(
            project_model,
            index_json,
            params,
            BTreeMap::new(),
            run_exports,
            package_run_exports,
        )];
    }

    // Get variant values for each key from the configuration
    let variant_values: Vec<(String, Vec<VariantValue>)> = variant_keys
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
            BTreeMap::new(),
            run_exports,
            package_run_exports,
        )];
    }

    // Generate all combinations of variant values
    let combinations = generate_variant_combinations(&variant_values);

    // Create an output for each variant combination
    combinations
        .into_iter()
        .map(|variant| {
            create_output(
                project_model,
                index_json,
                params,
                variant,
                run_exports,
                package_run_exports,
            )
        })
        .collect()
}

/// Finds all dependency names that have "*" requirements and have variant
/// configurations.
fn find_variant_keys(project_model: &ProjectModel, params: &CondaOutputsParams) -> Vec<String> {
    let Some(targets) = &project_model.targets else {
        return Vec::new();
    };

    let Some(variant_config) = &params.variant_configuration else {
        return Vec::new();
    };

    let mut variant_keys = BTreeSet::new();

    // Helper to check dependencies in a target
    let mut check_deps = |deps: Option<&OrderMap<SourcePackageName, PackageSpec>>| {
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
fn is_star_requirement(spec: &PackageSpec) -> bool {
    let PackageSpec::Binary(boxed) = spec else {
        return false;
    };

    match boxed {
        BinaryPackageSpec {
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
    variant_values: &[(String, Vec<VariantValue>)],
) -> Vec<BTreeMap<String, VariantValue>> {
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

/// Computes the build string for a package with optional variant hash.
///
/// When there are real variants (variants other than just target_platform),
/// the build string is augmented with a hash of the variant to ensure unique
/// package identities.
fn compute_build_string(
    base_build: &str,
    variant: &BTreeMap<String, VariantValue>,
    has_real_variants: bool,
) -> String {
    if !has_real_variants {
        base_build.to_string()
    } else {
        let variant_hash = compute_variant_hash(variant);
        if base_build.is_empty() {
            variant_hash
        } else {
            format!("{}_{}", base_build, variant_hash)
        }
    }
}

/// Computes a short hash of the variant for use in build strings.
/// This ensures that different variants produce different build strings.
fn compute_variant_hash(variant: &BTreeMap<String, VariantValue>) -> String {
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    for (key, value) in variant {
        key.hash(&mut hasher);
        value.to_string().hash(&mut hasher);
    }
    let hash = hasher.finish();
    // Use first 8 hex characters for a shorter, readable hash
    format!("{:08x}", hash as u32)
}

/// Creates a single output with the given variant configuration.
fn create_output(
    project_model: &ProjectModel,
    index_json: &IndexJson,
    params: &CondaOutputsParams,
    mut variant: BTreeMap<String, VariantValue>,
    run_exports_config: &BTreeMap<String, RunExportsJson>,
    package_run_exports: Option<&RunExportsJson>,
) -> CondaOutput {
    let subdir = index_json
        .subdir
        .clone()
        .map(|s| s.parse().unwrap())
        .unwrap_or(Platform::NoArch);

    // Track if there were actual variants before we add target_platform.
    // We only compute a build hash when there are real variants (not just target_platform).
    let has_real_variants = !variant.is_empty();

    // Always add target_platform for consistency
    if !variant.contains_key("target_platform") {
        variant.insert(
            String::from("target_platform"),
            VariantValue::from(subdir.to_string()),
        );
    }

    // Extract explicit run dependencies
    let mut run_dependencies = extract_dependencies(
        &project_model.targets,
        |t| t.run_dependencies.as_ref(),
        params.host_platform,
        &variant,
    );

    // Extract host dependencies
    let host_deps = extract_dependencies(
        &project_model.targets,
        |t| t.host_dependencies.as_ref(),
        params.host_platform,
        &variant,
    );

    // Apply run_exports from host dependencies.
    // For each host dependency that has run_exports configured, add the weak exports
    // to run_dependencies with variant values substituted.
    for host_dep in &host_deps.depends {
        if let Some(pkg_run_exports) = run_exports_config.get(host_dep.name.as_str()) {
            // Apply weak run_exports (most common case)
            for weak_export in &pkg_run_exports.weak {
                // Parse the run_export spec and substitute variant values
                if let Some(pinned_spec) = resolve_run_export_spec(weak_export, &variant) {
                    run_dependencies.depends.push(pinned_spec);
                }
            }
        }
    }

    CondaOutput {
        build_dependencies: Some(extract_dependencies(
            &project_model.targets,
            |t| t.build_dependencies.as_ref(),
            params.host_platform,
            &variant,
        )),
        host_dependencies: Some(host_deps),
        run_dependencies,
        metadata: CondaOutputMetadata {
            name: project_model
                .name
                .as_ref()
                .map(|name| PackageName::try_from(name.as_str()).unwrap())
                .unwrap_or_else(|| index_json.name.clone()),
            version: project_model
                .version
                .as_ref()
                .or_else(|| Some(index_json.version.version()))
                .cloned()
                .unwrap_or_else(|| Version::major(0))
                .into(),
            build: compute_build_string(&index_json.build, &variant, has_real_variants),
            build_number: index_json.build_number,
            subdir,
            license: project_model.license.clone(),
            license_family: None,
            noarch: index_json.noarch,
            purls: None,
            python_site_packages_path: None,
            variant,
        },
        ignore_run_exports: Default::default(),
        run_exports: package_run_exports
            .map(convert_run_exports_json)
            .unwrap_or_default(),
        input_globs: None,
    }
}

fn extract_dependencies<F: Fn(&Target) -> Option<&OrderMap<SourcePackageName, PackageSpec>>>(
    targets: &Option<Targets>,
    extract: F,
    platform: Platform,
    variant: &BTreeMap<String, VariantValue>,
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
                            PackageSpec::Binary(BinaryPackageSpec {
                                version: Some(
                                    rattler_conda_types::VersionSpec::from_str(
                                        variant_value.to_string().as_str(),
                                        rattler_conda_types::ParseStrictness::Lenient,
                                    )
                                    .unwrap(),
                                ),
                                ..Default::default()
                            })
                        } else {
                            spec.clone()
                        }
                    } else {
                        spec.clone()
                    };

                    NamedSpec {
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

/// Resolves a run_export spec string (like "sdl2 *") by substituting variant values.
///
/// If the spec contains a "*" version and there's a variant value for the package name,
/// the version is replaced with the variant value. Otherwise returns the spec as-is.
fn resolve_run_export_spec(
    run_export_str: &str,
    variant: &BTreeMap<String, VariantValue>,
) -> Option<NamedSpec<PackageSpec>> {
    // Parse the run_export string as a MatchSpec
    let match_spec = rattler_conda_types::MatchSpec::from_str(
        run_export_str,
        rattler_conda_types::ParseStrictness::Lenient,
    )
    .ok()?;

    let name = match_spec
        .name
        .as_ref()?
        .as_exact()?
        .as_source()
        .to_string();

    // Check if there's a variant value for this package
    let version_spec = if match_spec
        .version
        .as_ref()
        .is_none_or(|v| matches!(v, VersionSpec::Any))
    {
        // If version is "*" or unspecified, try to use the variant value
        if let Some(variant_value) = variant.get(&name) {
            Some(
                VersionSpec::from_str(
                    variant_value.to_string().as_str(),
                    rattler_conda_types::ParseStrictness::Lenient,
                )
                .ok()?,
            )
        } else {
            match_spec.version.clone()
        }
    } else {
        match_spec.version.clone()
    };

    Some(NamedSpec {
        name: SourcePackageName::from(name),
        spec: PackageSpec::Binary(BinaryPackageSpec {
            version: version_spec,
            ..Default::default()
        }),
    })
}

/// Converts a `RunExportsJson` (from a conda package) to `CondaOutputRunExports`.
fn convert_run_exports_json(
    run_exports: &RunExportsJson,
) -> pixi_build_types::procedures::conda_outputs::CondaOutputRunExports {
    fn convert_specs(specs: &[String]) -> Vec<NamedSpec<PackageSpec>> {
        specs
            .iter()
            .filter_map(|spec_str| {
                let match_spec = rattler_conda_types::MatchSpec::from_str(
                    spec_str,
                    rattler_conda_types::ParseStrictness::Lenient,
                )
                .ok()?;

                let name = match_spec
                    .name
                    .as_ref()?
                    .as_exact()?
                    .as_source()
                    .to_string();

                Some(NamedSpec {
                    name: SourcePackageName::from(name),
                    spec: PackageSpec::Binary(BinaryPackageSpec {
                        version: match_spec.version.clone(),
                        ..Default::default()
                    }),
                })
            })
            .collect()
    }

    fn convert_constraint_specs(specs: &[String]) -> Vec<NamedSpec<ConstraintSpec>> {
        specs
            .iter()
            .filter_map(|spec_str| {
                let match_spec = rattler_conda_types::MatchSpec::from_str(
                    spec_str,
                    rattler_conda_types::ParseStrictness::Lenient,
                )
                .ok()?;

                let name = match_spec
                    .name
                    .as_ref()?
                    .as_exact()?
                    .as_source()
                    .to_string();

                Some(NamedSpec {
                    name: SourcePackageName::from(name),
                    spec: ConstraintSpec::Binary(BinaryPackageSpec {
                        version: match_spec.version.clone(),
                        ..Default::default()
                    }),
                })
            })
            .collect()
    }

    pixi_build_types::procedures::conda_outputs::CondaOutputRunExports {
        weak: convert_specs(&run_exports.weak),
        strong: convert_specs(&run_exports.strong),
        noarch: convert_specs(&run_exports.noarch),
        weak_constrains: convert_constraint_specs(&run_exports.weak_constrains),
        strong_constrains: convert_constraint_specs(&run_exports.strong_constrains),
    }
}

/// Returns true if the given [`TargetSelector`] matches the specified
/// `platform`.
fn matches_target_selector(selector: &TargetSelector, platform: Platform) -> bool {
    match selector {
        TargetSelector::Unix => platform.is_unix(),
        TargetSelector::Linux => platform.is_linux(),
        TargetSelector::Win => platform.is_windows(),
        TargetSelector::MacOs => platform.is_osx(),
        TargetSelector::Platform(target_platform) => target_platform == platform.as_str(),
    }
}

/// An implementation of the [`InMemoryBackendInstantiator`] that creates a
/// [`PassthroughBackend`].
#[derive(Default)]
pub struct PassthroughBackendInstantiator {
    /// Run exports configuration for simulating package run_exports.
    /// Maps package names to their run_exports definitions.
    run_exports: BTreeMap<String, RunExportsJson>,
}

impl PassthroughBackendInstantiator {
    /// Adds run_exports configuration for a package.
    pub fn with_run_exports(
        mut self,
        package_name: impl Into<String>,
        run_exports: RunExportsJson,
    ) -> Self {
        self.run_exports.insert(package_name.into(), run_exports);
        self
    }
}

impl InMemoryBackendInstantiator for PassthroughBackendInstantiator {
    type Backend = PassthroughBackend;

    fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<Self::Backend, Box<CommunicationError>> {
        let project_model = match params.project_model {
            Some(project_model) => project_model,
            None => {
                return Err(Box::new(CommunicationError::BackendError(
                    BackendError::new("Passthrough backend requires a project model"),
                )));
            }
        };

        let config = match params.configuration {
            Some(config) => serde_json::from_value(config).expect("Failed to parse configuration"),
            None => PassthroughBackendConfig::default(),
        };

        // Read the package file if it is specified, or create IndexJson for on_the_fly mode
        let source_dir = params.source_directory.expect("Missing source directory");
        let (index_json, package_run_exports) = match &config.package {
            Some(path) => {
                let path = source_dir.join(path);
                let index_json: IndexJson =
                    match rattler_package_streaming::seek::read_package_file(&path) {
                        Err(err) => {
                            return Err(Box::new(
                                BackendError::new(format!(
                                    "failed to read index.json from '{}': {}",
                                    path.display(),
                                    err
                                ))
                                .into(),
                            ));
                        }
                        Ok(index_json) => index_json,
                    };
                // Also read run_exports.json from the package (optional, may not exist)
                let run_exports: Option<RunExportsJson> =
                    rattler_package_streaming::seek::read_package_file(&path).ok();
                (index_json, run_exports)
            }
            None => {
                // Create IndexJson from project model for on-the-fly package generation
                let index_json = IndexJson {
                    arch: None,
                    build: String::new(),
                    build_number: 0,
                    constrains: vec![],
                    depends: vec![],
                    experimental_extra_depends: Default::default(),
                    features: None,
                    license: project_model.license.clone(),
                    license_family: None,
                    name: project_model
                        .name
                        .as_ref()
                        .map(|n| PackageName::try_from(n.as_str()).unwrap())
                        .unwrap_or_else(|| PackageName::try_from("on-the-fly-package").unwrap()),
                    noarch: Default::default(),
                    platform: None,
                    purls: None,
                    python_site_packages_path: None,
                    subdir: None,
                    timestamp: None,
                    track_features: vec![],
                    version: project_model
                        .version
                        .clone()
                        .unwrap_or_else(|| Version::major(0))
                        .into(),
                };
                (index_json, None)
            }
        };

        Ok(PassthroughBackend {
            project_model,
            config,
            source_dir,
            index_json,
            run_exports: self.run_exports.clone(),
            package_run_exports,
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

    /// Whether this is a noarch package
    pub noarch: Option<bool>,

    /// Build globs
    pub build_globs: Option<BTreeSet<String>>,
}

/// Observer that allows collecting backend events from an ObservableBackend.
/// Use the `get_events` method to retrieve all available events.
pub struct BackendObserver {
    receiver: tokio::sync::mpsc::UnboundedReceiver<BackendEvent>,
}

impl BackendObserver {
    /// Collects all available events from the channel using try_recv.
    /// This is non-blocking and returns immediately with all events that
    /// are currently in the channel.
    pub fn events(&mut self) -> Vec<BackendEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        events
    }

    /// Collects all build events from the channel using try_recv.
    /// This is non-blocking and returns immediately with all events that
    /// are currently in the channel.
    pub fn build_events(&mut self) -> Vec<BackendEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            if matches!(event, BackendEvent::CondaBuildV1Called) {
                events.push(event);
            }
        }
        events
    }

    /// Waits for build events with a timeout.
    /// Returns all build events received within the timeout period.
    /// This is useful for tests where the build might happen asynchronously.
    pub async fn wait_for_build_events(
        &mut self,
        timeout: std::time::Duration,
    ) -> Vec<BackendEvent> {
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, self.receiver.recv()).await {
                Ok(Some(event)) => {
                    if matches!(event, BackendEvent::CondaBuildV1Called) {
                        events.push(event);
                    }
                }
                Ok(None) => break, // Channel closed
                Err(_) => break,   // Timeout
            }
        }

        events
    }
}

/// An observable wrapper around any InMemoryBackend that emits events through a
/// channel when methods are called. This is useful for testing to verify that
/// specific backend methods were invoked.
pub struct ObservableBackend<T: InMemoryBackend> {
    inner: T,
    event_sender: tokio::sync::mpsc::UnboundedSender<BackendEvent>,
}

impl<T: InMemoryBackend> ObservableBackend<T> {
    /// Creates a new instantiator for an ObservableBackend wrapping the given
    /// backend. Returns both the instantiator and a BackendObserver for
    /// collecting events.
    pub fn instantiator<I>(
        inner_instantiator: I,
    ) -> (
        impl InMemoryBackendInstantiator<Backend = Self>,
        BackendObserver,
    )
    where
        I: InMemoryBackendInstantiator<Backend = T>,
    {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<BackendEvent>();
        let observer = BackendObserver { receiver: rx };
        let instantiator = ObservableBackendInstantiator {
            inner_instantiator,
            event_sender: tx,
        };
        (instantiator, observer)
    }
}

impl<T: InMemoryBackend> InMemoryBackend for ObservableBackend<T> {
    fn capabilities(&self) -> BackendCapabilities {
        self.inner.capabilities()
    }

    fn identifier(&self) -> &str {
        self.inner.identifier()
    }

    fn conda_outputs(
        &self,
        params: CondaOutputsParams,
        output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaOutputsResult, Box<CommunicationError>> {
        // Emit event
        let _ = self.event_sender.send(BackendEvent::CondaOutputsCalled);

        // Delegate to the inner backend
        self.inner.conda_outputs(params, output_stream)
    }

    fn conda_build_v1(
        &self,
        params: CondaBuildV1Params,
        output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaBuildV1Result, Box<CommunicationError>> {
        // Emit event
        let _ = self.event_sender.send(BackendEvent::CondaBuildV1Called);

        // Delegate to the inner backend
        self.inner.conda_build_v1(params, output_stream)
    }
}

/// An implementation of the [`InMemoryBackendInstantiator`] that creates an
/// [`ObservableBackend`] wrapping any other backend.
pub struct ObservableBackendInstantiator<I, T>
where
    I: InMemoryBackendInstantiator<Backend = T>,
    T: InMemoryBackend,
{
    inner_instantiator: I,
    event_sender: tokio::sync::mpsc::UnboundedSender<BackendEvent>,
}

impl<I, T> InMemoryBackendInstantiator for ObservableBackendInstantiator<I, T>
where
    I: InMemoryBackendInstantiator<Backend = T>,
    T: InMemoryBackend,
{
    type Backend = ObservableBackend<T>;

    fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<Self::Backend, Box<CommunicationError>> {
        let inner = self.inner_instantiator.initialize(params)?;
        Ok(ObservableBackend {
            inner,
            event_sender: self.event_sender.clone(),
        })
    }

    fn identifier(&self) -> &str {
        self.inner_instantiator.identifier()
    }
}

#[cfg(test)]
mod tests {
    use pixi_build_types::{BinaryPackageSpec, PackageSpec};
    use rattler_conda_types::{ParseStrictness, VersionSpec};

    use super::*;

    #[test]
    fn test_is_star_requirement_with_star() {
        let spec = PackageSpec::Binary(BinaryPackageSpec {
            version: Some(VersionSpec::from_str("*", ParseStrictness::Lenient).unwrap()),
            ..Default::default()
        });

        assert!(is_star_requirement(&spec));
    }

    #[test]
    fn test_is_star_requirement_with_version() {
        let spec = PackageSpec::Binary(BinaryPackageSpec {
            version: Some(VersionSpec::from_str(">=1.0", ParseStrictness::Lenient).unwrap()),
            ..Default::default()
        });

        assert!(!is_star_requirement(&spec));
    }

    #[test]
    fn test_is_star_requirement_with_no_version() {
        let spec = PackageSpec::Binary(BinaryPackageSpec::default());

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
            vec![
                VariantValue::String("3.10".to_string()),
                VariantValue::String("3.11".to_string()),
            ],
        )]);

        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].get("python").unwrap().to_string(), "3.10");
        assert_eq!(variants[1].get("python").unwrap().to_string(), "3.11");
    }

    #[test]
    fn test_generate_variant_combinations_multiple() {
        let variants = generate_variant_combinations(&[
            (
                "python".to_string(),
                vec![
                    VariantValue::String("3.10".to_string()),
                    VariantValue::String("3.11".to_string()),
                ],
            ),
            (
                "numpy".to_string(),
                vec![
                    VariantValue::String("1.0".to_string()),
                    VariantValue::String("2.0".to_string()),
                ],
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
                    .any(|v| v.get("python").unwrap().to_string() == expected_python
                        && v.get("numpy").unwrap().to_string() == expected_numpy),
                "Expected combination ({expected_python}, {expected_numpy}) not found"
            );
        }
    }

    #[test]
    fn test_generate_variant_combinations_three_dimensions() {
        let variants = generate_variant_combinations(&[
            (
                "python".to_string(),
                vec![
                    VariantValue::String("3.10".to_string()),
                    VariantValue::String("3.11".to_string()),
                ],
            ),
            (
                "numpy".to_string(),
                vec![
                    VariantValue::String("1.0".to_string()),
                    VariantValue::String("2.0".to_string()),
                ],
            ),
            (
                "os".to_string(),
                vec![
                    VariantValue::String("linux".to_string()),
                    VariantValue::String("windows".to_string()),
                ],
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
            (
                "python".to_string(),
                vec![VariantValue::String("3.10".to_string())],
            ),
            (
                "numpy".to_string(),
                vec![VariantValue::String("1.0".to_string())],
            ),
        ]);

        // Should generate only 1 combination
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].get("python").unwrap().to_string(), "3.10");
        assert_eq!(variants[0].get("numpy").unwrap().to_string(), "1.0");
    }
}
