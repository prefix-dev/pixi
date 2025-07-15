use ordermap::OrderMap;
use pixi_build_types::{
    BackendCapabilities, NamedSpecV1, PackageSpecV1, ProjectModelV1, SourcePackageName,
    TargetSelectorV1, TargetV1, TargetsV1, VersionedProjectModel,
    procedures::{
        conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputMetadata, CondaOutputsParams,
            CondaOutputsResult,
        },
        initialize::InitializeParams,
    },
};
use rattler_conda_types::{NoArchType, Platform, Version};

use crate::{
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
}

impl PassthroughBackend {
    /// Returns a new instance of the [`PassthroughBackendInstantiator`] which
    /// can be used to instantiate a [`PassthroughBackend`].
    pub fn instantiator() -> PassthroughBackendInstantiator {
        PassthroughBackendInstantiator
    }
}

impl InMemoryBackend for PassthroughBackend {
    fn capabilities(&self) -> BackendCapabilities {
        BackendCapabilities {
            provides_conda_outputs: Some(true),
            ..BackendCapabilities::default()
        }
    }

    fn identifier(&self) -> &'static str {
        BACKEND_NAME
    }

    fn conda_outputs(
        &self,
        params: CondaOutputsParams,
    ) -> Result<CondaOutputsResult, CommunicationError> {
        Ok(CondaOutputsResult {
            outputs: vec![CondaOutput {
                metadata: CondaOutputMetadata {
                    name: self.project_model.name.parse().unwrap(),
                    version: self
                        .project_model
                        .version
                        .clone()
                        .unwrap_or_else(|| Version::major(0))
                        .into(),
                    build: String::new(),
                    build_number: 0,
                    subdir: Platform::NoArch,
                    license: self.project_model.license.clone(),
                    license_family: None,
                    noarch: NoArchType::generic(),
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
                            if match selector {
                                TargetSelectorV1::Unix => platform.is_unix(),
                                TargetSelectorV1::Linux => platform.is_linux(),
                                TargetSelectorV1::Win => platform.is_windows(),
                                TargetSelectorV1::MacOs => platform.is_osx(),
                                TargetSelectorV1::Platform(target_platform) => {
                                    target_platform == platform.as_str()
                                }
                            } {
                                Some(target)
                            } else {
                                None
                            }
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
        Ok(PassthroughBackend { project_model })
    }

    fn identifier(&self) -> &'static str {
        BACKEND_NAME
    }
}
