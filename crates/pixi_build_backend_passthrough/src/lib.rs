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
    BackendCapabilities, BinaryPackageSpecV1, NamedSpecV1, PackageSpecV1, ProjectModelV1,
    SourcePackageName, TargetSelectorV1, TargetV1, TargetsV1, VariantValue, VersionedProjectModel,
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
    package::{IndexJson, PathType, PathsEntry, PathsJson},
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
    project_model: ProjectModelV1,
    config: PassthroughBackendConfig,
    source_dir: PathBuf,
    index_json: IndexJson,
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
        _output_stream: &(dyn BackendOutputStream + Send + 'static),
    ) -> Result<CondaOutputsResult, Box<CommunicationError>> {
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
    ) -> Result<CondaBuildV1Result, Box<CommunicationError>> {
        // Compute the variant-aware build string (must match what conda_outputs returns)
        let variant: BTreeMap<String, VariantValue> = params
            .output
            .variant
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let build_string = if variant.is_empty() {
            self.index_json.build.clone()
        } else {
            let variant_hash = compute_variant_hash(&variant);
            if self.index_json.build.is_empty() {
                variant_hash
            } else {
                format!("{}_{}", self.index_json.build, variant_hash)
            }
        };

        let output_dir = params
            .output_directory
            .unwrap_or(params.work_directory.clone());

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

                // Create a modified index_json with the variant-aware build string
                let mut modified_index_json = self.index_json.clone();
                modified_index_json.build = build_string.clone();

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
            subdir: self
                .index_json
                .subdir
                .as_ref()
                .expect("missing subdir in index.json")
                .parse()
                .expect("invalid subdir in index.json"),
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

/// Generates all variant outputs for a package based on the variant configuration.
///
/// If any dependency has a "*" version requirement and there's a variant configuration
/// for that package, multiple outputs will be generated - one for each variant combination.
fn generate_variant_outputs(
    project_model: &ProjectModelV1,
    index_json: &IndexJson,
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
            BTreeMap::new(),
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
        )];
    }

    // Generate all combinations of variant values
    let combinations = generate_variant_combinations(&variant_values);

    // Create an output for each variant combination
    combinations
        .into_iter()
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
    project_model: &ProjectModelV1,
    index_json: &IndexJson,
    params: &CondaOutputsParams,
    mut variant: BTreeMap<String, VariantValue>,
) -> CondaOutput {
    let subdir = index_json
        .subdir
        .clone()
        .map(|s| s.parse().unwrap())
        .unwrap_or(Platform::NoArch);

    if !variant.contains_key("target_platform") {
        variant.insert(
            String::from("target_platform"),
            VariantValue::from(subdir.to_string()),
        );
    }

    CondaOutput {
        build_dependencies: Some(extract_dependencies(
            &project_model.targets,
            |t| t.build_dependencies.as_ref(),
            params.host_platform,
            &variant,
        )),
        host_dependencies: Some(extract_dependencies(
            &project_model.targets,
            |t| t.host_dependencies.as_ref(),
            params.host_platform,
            &variant,
        )),
        run_dependencies: extract_dependencies(
            &project_model.targets,
            |t| t.run_dependencies.as_ref(),
            params.host_platform,
            &variant,
        ),
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
            build: {
                let base_build = index_json.build.clone();
                // If there are variants, append a hash to make the build string unique
                if variant.is_empty() {
                    base_build
                } else {
                    let variant_hash = compute_variant_hash(&variant);
                    if base_build.is_empty() {
                        variant_hash
                    } else {
                        format!("{}_{}", base_build, variant_hash)
                    }
                }
            },
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
        run_exports: Default::default(),
        input_globs: None,
    }
}

fn extract_dependencies<F: Fn(&TargetV1) -> Option<&OrderMap<SourcePackageName, PackageSpecV1>>>(
    targets: &Option<TargetsV1>,
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
                            PackageSpecV1::Binary(Box::new(BinaryPackageSpecV1 {
                                version: Some(
                                    rattler_conda_types::VersionSpec::from_str(
                                        variant_value.to_string().as_str(),
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

    fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<Self::Backend, Box<CommunicationError>> {
        let project_model = match params.project_model {
            Some(VersionedProjectModel::V1(project_model)) => project_model,
            _ => {
                return Err(Box::new(CommunicationError::BackendError(
                    BackendError::new("Passthrough backend only supports project model v1"),
                )));
            }
        };

        let config = match params.configuration {
            Some(config) => serde_json::from_value(config).expect("Failed to parse configuration"),
            None => PassthroughBackendConfig::default(),
        };

        // Read the package file if it is specified, or create IndexJson for on_the_fly mode
        let source_dir = params.source_dir.expect("Missing source directory");
        let index_json = match &config.package {
            Some(path) => {
                let path = source_dir.join(path);
                match rattler_package_streaming::seek::read_package_file(&path) {
                    Err(err) => {
                        return Err(Box::new(
                            BackendError::new(format!(
                                "failed to read '{}' file: {}",
                                path.display(),
                                err
                            ))
                            .into(),
                        ));
                    }
                    Ok(index_json) => index_json,
                }
            }
            None => {
                // Create IndexJson from project model for on-the-fly package generation
                IndexJson {
                    arch: None,
                    build: String::from("0"),
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
                    subdir: Some(Platform::current().to_string()),
                    timestamp: None,
                    track_features: vec![],
                    version: project_model
                        .version
                        .clone()
                        .unwrap_or_else(|| Version::major(0))
                        .into(),
                }
            }
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
}

/// An observable wrapper around any InMemoryBackend that emits events through a
/// channel when methods are called. This is useful for testing to verify that
/// specific backend methods were invoked.
pub struct ObservableBackend<T: InMemoryBackend> {
    inner: T,
    event_sender: tokio::sync::mpsc::UnboundedSender<BackendEvent>,
}

impl<T: InMemoryBackend> ObservableBackend<T> {
    /// Creates a new instantiator for an ObservableBackend wrapping the given backend.
    /// Returns both the instantiator and a BackendObserver for collecting events.
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
