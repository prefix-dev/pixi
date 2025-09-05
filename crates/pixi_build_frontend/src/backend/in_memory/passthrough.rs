use std::{collections::BTreeSet, path::PathBuf};

use ordermap::OrderMap;
use pixi_build_types::{
    BackendCapabilities, NamedSpecV1, PackageSpecV1, ProjectModelV1, SourcePackageName,
    TargetSelectorV1, TargetV1, TargetsV1, VersionedProjectModel,
    procedures::{
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputMetadata, CondaOutputsParams,
            CondaOutputsResult,
        },
        initialize::InitializeParams,
    },
};
use rattler_conda_types::{PackageName, Platform, Version, package::IndexJson};
use serde::Deserialize;

use crate::{
    BackendOutputStream,
    error::BackendError,
    in_memory::{InMemoryBackend, InMemoryBackendInstantiator},
    json_rpc::CommunicationError,
};

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
        Ok(CondaOutputsResult {
            outputs: vec![CondaOutput {
                metadata: CondaOutputMetadata {
                    name: self
                        .project_model
                        .name
                        .as_ref()
                        .map(|name| PackageName::try_from(name.as_str()).unwrap())
                        .unwrap_or_else(|| {
                            self.index_json
                                .as_ref()
                                .map(|j| j.name.clone())
                                .unwrap_or_else(|| {
                                    PackageName::try_from("pixi-package_name").unwrap()
                                })
                        }),
                    version: self
                        .project_model
                        .version
                        .as_ref()
                        .or_else(|| self.index_json.as_ref().map(|j| j.version.version()))
                        .cloned()
                        .unwrap_or_else(|| Version::major(0))
                        .into(),
                    build: self
                        .index_json
                        .as_ref()
                        .map(|j| j.build.clone())
                        .unwrap_or_default(),
                    build_number: self
                        .index_json
                        .as_ref()
                        .map(|j| j.build_number)
                        .unwrap_or_default(),
                    subdir: self
                        .index_json
                        .as_ref()
                        .and_then(|j| j.subdir.as_deref())
                        .map(|subdir| subdir.parse().unwrap())
                        .unwrap_or(Platform::NoArch),
                    license: self.project_model.license.clone(),
                    license_family: None,
                    noarch: self
                        .index_json
                        .as_ref()
                        .map(|j| j.noarch)
                        .unwrap_or_default(),
                    purls: None,
                    python_site_packages_path: None,
                    variant: Default::default(),
                },
                build_dependencies: Some(extract_dependencies(
                    &self.project_model.targets,
                    |t| t.build_dependencies.as_ref(),
                    params.host_platform,
                )),
                host_dependencies: Some(extract_dependencies(
                    &self.project_model.targets,
                    |t| t.host_dependencies.as_ref(),
                    params.host_platform,
                )),
                run_dependencies: extract_dependencies(
                    &self.project_model.targets,
                    |t| t.run_dependencies.as_ref(),
                    params.host_platform,
                ),
                ignore_run_exports: Default::default(),
                run_exports: Default::default(),
                input_globs: None,
            }],
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

fn extract_dependencies<F: Fn(&TargetV1) -> Option<&OrderMap<SourcePackageName, PackageSpecV1>>>(
    targets: &Option<TargetsV1>,
    extract: F,
    platform: Platform,
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
                .map(|(name, spec)| NamedSpecV1 {
                    name: name.clone(),
                    spec: spec.clone(),
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
