use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{config::RattlerBuildBackendConfig, rattler_build::RattlerBuildBackend};
use miette::{Context, IntoDiagnostic};
use pixi_build_backend::specs_conversion::{
    convert_variant_from_pixi_build_types, convert_variant_to_pixi_build_types,
    from_build_v1_args_to_finalized_dependencies,
};
use pixi_build_backend::{
    dependencies::{convert_constraint_dependencies, convert_dependencies},
    intermediate_backend::{conda_build_v1_directories, find_matching_output},
    protocol::{Protocol, ProtocolInstantiator},
    tools::LoadedVariantConfig,
};
use pixi_build_types::{
    BackendCapabilities, PathSpec, SourcePackageSpec, Target,
    procedures::{
        conda_build_v1::{CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputIgnoreRunExports, CondaOutputMetadata,
            CondaOutputRunExports, CondaOutputsParams, CondaOutputsResult,
        },
        initialize::{InitializeParams, InitializeResult},
        negotiate_capabilities::{NegotiateCapabilitiesParams, NegotiateCapabilitiesResult},
    },
};
use rattler_build::{
    build::{WorkingDirectoryBehavior, run_build},
    console_utils::LoggingOutputHandler,
    hash::HashInfo,
    metadata::{BuildConfiguration, Debug, Output, PlatformWithVirtualPackages},
    recipe::{ParsingError, Recipe, parser::find_outputs_from_src},
    selectors::SelectorConfig,
    tool_configuration::Configuration,
    types::{PackageIdentifier, PackagingSettings},
    variant_config::{ParseErrors, VariantConfig},
};
use rattler_conda_types::{Platform, compression_level::CompressionLevel, package::ArchiveType};
use tracing::warn;
pub struct RattlerBuildBackendInstantiator {
    logging_output_handler: LoggingOutputHandler,
}

impl RattlerBuildBackendInstantiator {
    /// This type implements [`ProtocolInstantiator`] and can be used to
    /// initialize a new [`RattlerBuildBackend`].
    pub fn new(logging_output_handler: LoggingOutputHandler) -> RattlerBuildBackendInstantiator {
        RattlerBuildBackendInstantiator {
            logging_output_handler,
        }
    }
}

#[async_trait::async_trait]
impl Protocol for RattlerBuildBackend {
    async fn conda_outputs(
        &self,
        params: CondaOutputsParams,
    ) -> miette::Result<CondaOutputsResult> {
        let build_platform = params.host_platform;

        // Determine the variant configuration to use. This loads the variant
        // configuration from disk as well as including the variants from the input
        // parameters.
        let selector_config_for_variants = SelectorConfig {
            target_platform: params.host_platform,
            host_platform: params.host_platform,
            build_platform,
            hash: None,
            variant: Default::default(),
            experimental: self.config.experimental.unwrap_or(false),
            allow_undefined: false,
            recipe_path: Some(self.recipe_source.path.clone()),
        };
        let variant_config = LoadedVariantConfig::from_recipe_path(
            &self.source_dir,
            &self.recipe_source.path,
            &selector_config_for_variants,
            params.variant_files.iter().flatten().map(PathBuf::as_path),
        )?
        .extend_with_input_variants(params.variant_configuration.unwrap_or_default());

        // Find all outputs from the recipe
        let output_nodes = find_outputs_from_src(self.recipe_source.clone())?;
        let discovered_outputs = variant_config.variant_config.find_variants(
            &output_nodes,
            self.recipe_source.clone(),
            &selector_config_for_variants,
        )?;

        // Construct a mapping that for packages that we want from source.
        //
        // By default, this includes all the outputs in the recipe. These should all be
        // build from source, in particular from the current source.
        let mut local_source_packages: HashMap<String, SourcePackageSpec> = discovered_outputs
            .iter()
            .map(|output| (output.name.clone(), PathSpec { path: ".".into() }.into()))
            .collect();

        // Add workspace dependencies to the source packages mapping.
        // This allows the recipe to reference workspace packages by name (e.g., "my-lib")
        // and have them automatically resolved to source dependencies with the correct path.
        local_source_packages.extend(self.workspace_dependencies.clone());

        let mut subpackages = HashMap::new();
        let mut outputs = Vec::new();
        for discovered_output in discovered_outputs {
            let variant = discovered_output.used_vars;
            let hash = HashInfo::from_variant(&variant, &discovered_output.noarch_type);

            // Construct the selector config for this particular output. We base this on the
            // selector config that was used to determine the variants.
            let selector_config = SelectorConfig {
                variant: variant.clone(),
                hash: Some(hash.clone()),
                target_platform: discovered_output.target_platform,
                ..selector_config_for_variants.clone()
            };

            // Convert this discovered output into a recipe.
            let recipe = Recipe::from_node(&discovered_output.node, selector_config.clone())
                .map_err(|err| {
                    let errs: ParseErrors<_> = err
                        .into_iter()
                        .map(|err| ParsingError::from_partial(self.recipe_source.clone(), err))
                        .collect::<Vec<_>>()
                        .into();
                    errs
                })?;

            // Skip this output if the recipe is marked as skipped
            if recipe.build().skip() {
                continue;
            }

            subpackages.insert(
                recipe.package().name().clone(),
                PackageIdentifier {
                    name: recipe.package().name().clone(),
                    version: recipe.package().version().version().clone().into(),
                    build_string: discovered_output.build_string.clone(),
                },
            );

            outputs.push(CondaOutput {
                metadata: CondaOutputMetadata {
                    name: recipe.package().name().clone(),
                    version: recipe.package.version().clone(),
                    build: discovered_output.build_string.clone(),
                    build_number: recipe.build().number,
                    subdir: discovered_output.target_platform,
                    license: recipe.about.license.map(|l| l.to_string()),
                    license_family: recipe.about.license_family,
                    noarch: recipe.build.noarch,
                    purls: None,
                    python_site_packages_path: recipe.build.python.site_packages_path.clone(),
                    variant: variant
                        .iter()
                        .map(|(key, value)| {
                            Ok((
                                key.0.clone(),
                                convert_variant_to_pixi_build_types(value.clone()).into_diagnostic()
                                    .with_context(|| {
                                        format!("the output {}/{}={}={} contains a variant for '{}' which cannot be converted to pixi types: {}",
                                            discovered_output.target_platform,
                                            discovered_output.name,
                                            discovered_output.version,
                                            discovered_output.build_string,
                                            key.0,
                                            value)
                                    })?
                            ))
                        })
                        .collect::<miette::Result<_>>()?,
                },
                build_dependencies: Some(CondaOutputDependencies {
                    depends: convert_dependencies(
                        recipe.requirements.build,
                        &variant,
                        &subpackages,
                        &local_source_packages,
                    )?,
                    constraints: Vec::new(),
                }),
                host_dependencies: Some(CondaOutputDependencies {
                    depends: convert_dependencies(
                        recipe.requirements.host,
                        &variant,
                        &subpackages,
                        &local_source_packages,
                    )?,
                    constraints: Vec::new(),
                }),
                run_dependencies: CondaOutputDependencies {
                    depends: convert_dependencies(
                        recipe.requirements.run,
                        &BTreeMap::default(), // Variants are not applied to run dependencies
                        &subpackages,
                        &local_source_packages,
                    )?,
                    constraints: convert_constraint_dependencies(
                        recipe.requirements.run_constraints,
                        &BTreeMap::default(), // Variants are not applied to run constraints
                        &subpackages,
                    )?,
                },
                ignore_run_exports: CondaOutputIgnoreRunExports {
                    by_name: recipe
                        .requirements
                        .ignore_run_exports
                        .by_name
                        .into_iter()
                        .collect(),
                    from_package: recipe
                        .requirements
                        .ignore_run_exports
                        .from_package
                        .into_iter()
                        .collect(),
                },
                run_exports: CondaOutputRunExports {
                    weak: convert_dependencies(
                        recipe.requirements.run_exports.weak,
                        &variant,
                        &subpackages,
                        &local_source_packages,
                    )?,
                    strong: convert_dependencies(
                        recipe.requirements.run_exports.strong,
                        &variant,
                        &subpackages,
                        &local_source_packages,
                    )?,
                    noarch: convert_dependencies(
                        recipe.requirements.run_exports.noarch,
                        &variant,
                        &subpackages,
                        &local_source_packages,
                    )?,
                    weak_constrains: convert_constraint_dependencies(
                        recipe.requirements.run_exports.weak_constraints,
                        &variant,
                        &subpackages,
                    )?,
                    strong_constrains: convert_constraint_dependencies(
                        recipe.requirements.run_exports.strong_constraints,
                        &variant,
                        &subpackages,
                    )?,
                },

                // The input globs are the same for all outputs
                input_globs: None,
                // TODO: Implement caching
            });
        }

        let mut input_globs = variant_config.input_globs;
        input_globs.extend(get_metadata_input_globs(
            &self.manifest_root,
            &self.recipe_source.path,
        )?);

        Ok(CondaOutputsResult {
            outputs,
            input_globs,
        })
    }

    async fn conda_build_v1(
        &self,
        params: CondaBuildV1Params,
    ) -> miette::Result<CondaBuildV1Result> {
        let host_platform = params
            .host_prefix
            .as_ref()
            .map_or_else(Platform::current, |prefix| prefix.platform);
        let build_platform = params
            .build_prefix
            .as_ref()
            .map_or_else(Platform::current, |prefix| prefix.platform);

        // Construct a `VariantConfig` based on the input parameters. We only
        // have a single variant here so we can just use the variant from the
        // parameters.
        let variant_config = VariantConfig {
            variants: params
                .output
                .variant
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().into(),
                        vec![convert_variant_from_pixi_build_types(v.clone())],
                    )
                })
                .collect(),
            pin_run_as_build: None,
            zip_keys: None,
        };

        // Determine the variant configuration to use. This loads the variant
        // configuration from disk as well as including the variants from the input
        // parameters.
        let selector_config_for_variants = SelectorConfig {
            target_platform: host_platform,
            host_platform,
            build_platform,
            hash: None,
            variant: Default::default(),
            experimental: self.config.experimental.unwrap_or(false),
            allow_undefined: false,
            recipe_path: Some(self.recipe_source.path.clone()),
        };
        let outputs = find_outputs_from_src(self.recipe_source.clone())?;
        let discovered_outputs = variant_config.find_variants(
            &outputs,
            self.recipe_source.clone(),
            &selector_config_for_variants,
        )?;
        let discovered_output = find_matching_output(&params.output, discovered_outputs)?;

        // Set up the proper directories for the build.
        let directories = conda_build_v1_directories(
            params.host_prefix.as_ref().map(|p| p.prefix.as_path()),
            params.build_prefix.as_ref().map(|p| p.prefix.as_path()),
            params.work_directory,
            self.cache_dir.as_deref(),
            params.output_directory.as_deref(),
            self.recipe_source.path.clone(),
        );

        let tool_config = Configuration::builder()
            .with_opt_cache_dir(self.cache_dir.clone())
            .with_logging_output_handler(self.logging_output_handler.clone())
            .with_testing(false)
            // Pixi is incremental so keep the build
            .with_keep_build(true)
            // This indicates that the environments are externally managed, e.g. they are already
            // prepared.
            .with_environments_externally_managed(true)
            .finish();

        let output = Output {
            recipe: discovered_output.recipe,
            build_configuration: BuildConfiguration {
                target_platform: discovered_output.target_platform,
                host_platform: PlatformWithVirtualPackages {
                    platform: host_platform,
                    virtual_packages: vec![],
                },
                build_platform: PlatformWithVirtualPackages {
                    platform: build_platform,
                    virtual_packages: vec![],
                },
                hash: discovered_output.hash,
                variant: discovered_output.used_vars.clone(),
                directories,
                channels: vec![],
                channel_priority: Default::default(),
                solve_strategy: Default::default(),
                timestamp: chrono::Utc::now(),
                subpackages: BTreeMap::new(),
                packaging_settings: PackagingSettings::from_args(
                    ArchiveType::Conda,
                    CompressionLevel::default(),
                ),
                store_recipe: false,
                force_colors: true,
                sandbox_config: None,
                debug: Debug::new(false),
                exclude_newer: None,
            },
            finalized_dependencies: Some(from_build_v1_args_to_finalized_dependencies(
                params.build_prefix,
                params.host_prefix,
                params.run_dependencies,
                params.run_constraints,
                params.run_exports,
            )),
            finalized_sources: None,
            finalized_cache_dependencies: None,
            finalized_cache_sources: None,
            build_summary: Arc::default(),
            system_tools: Default::default(),
            extra_meta: None,
        };

        let (output, output_path) =
            // rattler-build requires a clean work dir
            run_build(output, &tool_config, WorkingDirectoryBehavior::Cleanup).await?;

        Ok(CondaBuildV1Result {
            output_file: output_path,
            input_globs: build_input_globs(
                &self.manifest_root,
                &self.recipe_source.path,
                extract_mutable_package_sources(&output),
                self.config.extra_input_globs.clone(),
            )?,
            name: output.name().as_normalized().to_string(),
            version: output.version().clone(),
            build: output.build_string().into_owned(),
            subdir: *output.target_platform(),
        })
    }
}

/// Extracts the package sources from an `Output` object that are mutable and
/// should be watched for changes.
fn extract_mutable_package_sources(output: &Output) -> Option<Vec<PathBuf>> {
    output.finalized_sources.as_ref().map(|package_sources| {
        package_sources
            .iter()
            .filter_map(|source| {
                if let rattler_build::recipe::parser::Source::Path(path_source) = source {
                    Some(path_source.path.clone())
                } else {
                    None
                }
            })
            .collect()
    })
}

/// Returns the relative path from `base` to `input`, joined by "/".
fn build_relative_glob(base: &std::path::Path, input: &std::path::Path) -> miette::Result<String> {
    // Get the difference between paths
    let rel = pathdiff::diff_paths(input, base).ok_or_else(|| {
        miette::miette!(
            "could not compute relative path from '{:?}' to '{:?}'",
            input,
            base
        )
    })?;

    // Normalize the path
    let joined = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    if input.is_dir() {
        // This means the base is the same as the input
        // just use `**` in that case that matches everything
        if joined.is_empty() {
            Ok("**".to_string())
        } else {
            Ok(format!("{joined}/**"))
        }
    } else {
        // This is a file so lets just use that
        Ok(joined)
    }
}

fn build_input_globs(
    manifest_root: &Path,
    source: &Path,
    package_sources: Option<Vec<PathBuf>>,
    extra_globs: Vec<String>,
) -> miette::Result<BTreeSet<String>> {
    // Get parent directory path
    let src_parent = if source.is_file() {
        // use the parent path as glob
        source.parent().unwrap_or(source).to_path_buf()
    } else {
        // use the source path as glob
        source.to_path_buf()
    };

    // Always add the current directory of the package to the globs
    let mut input_globs = BTreeSet::from([build_relative_glob(manifest_root, &src_parent)?]);

    // If there are sources add them to the globs as well
    if let Some(package_sources) = package_sources {
        for source in package_sources {
            let source = if source.is_absolute() {
                source
            } else {
                src_parent.join(source)
            };
            input_globs.insert(build_relative_glob(manifest_root, &source)?);
        }
    }

    // Extend with extra input globs
    input_globs.extend(extra_globs);

    Ok(input_globs)
}

/// Returns the input globs for conda_get_metadata, as used in the
/// CondaMetadataResult.
fn get_metadata_input_globs(
    manifest_root: &Path,
    recipe_source_path: &Path,
) -> miette::Result<BTreeSet<String>> {
    match build_relative_glob(manifest_root, recipe_source_path) {
        Ok(rel) if !rel.is_empty() => Ok(BTreeSet::from_iter([rel])),
        Ok(_) => Ok(Default::default()),
        Err(e) => Err(e),
    }
}

#[async_trait::async_trait]
impl ProtocolInstantiator for RattlerBuildBackendInstantiator {
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> miette::Result<(Box<dyn Protocol + Send + Sync + 'static>, InitializeResult)> {
        let config = if let Some(config) = params.configuration {
            serde_json::from_value(config)
                .into_diagnostic()
                .context("failed to parse configuration")?
        } else {
            RattlerBuildBackendConfig::default()
        };

        if let Some(path) = config.debug_dir.as_ref() {
            warn!(
                path = %path.display(),
                "`debug-dir` backend configuration is deprecated and ignored; debug data is now written to the build work directory."
            );
        }

        let mut workspace_dependencies = HashMap::new();

        if let Some(target) = params.project_model.and_then(|m| m.targets) {
            fn extract_workspace_deps(
                target: Target,
                workspace_deps: &mut HashMap<String, SourcePackageSpec>,
            ) -> miette::Result<()> {
                for dep_list in [
                    target.build_dependencies,
                    target.host_dependencies,
                    target.run_dependencies,
                ] {
                    let Some(deps) = dep_list else {
                        continue;
                    };

                    for (name, spec) in deps {
                        match spec {
                            pixi_build_types::PackageSpec::Source(source_spec) => {
                                // Source dependencies are allowed - they represent workspace packages
                                workspace_deps.insert(name, source_spec);
                            }
                            pixi_build_types::PackageSpec::Binary(_) => {
                                // Binary dependencies must be specified in the recipe, not here
                                return Err(miette::miette!(
                                    "Binary dependency '{}' is not allowed in pixi-build-rattler-build. Please specify all binary dependencies in the recipe.",
                                    name
                                ));
                            }
                            pixi_build_types::PackageSpec::PinCompatible(_) => {
                                // PinCompatible dependencies are not yet supported
                                return Err(miette::miette!(
                                    "PinCompatible dependency '{}' is not yet supported in pixi-build-rattler-build.",
                                    name
                                ));
                            }
                        }
                    }
                }
                Ok(())
            }

            if let Some(default_target) = target.default_target {
                extract_workspace_deps(default_target, &mut workspace_dependencies)?;
            }

            if let Some(targets) = target.targets {
                for (_, target) in targets {
                    extract_workspace_deps(target, &mut workspace_dependencies)?;
                }
            }
        }

        let mut instance = RattlerBuildBackend::new(
            params.source_directory,
            params.manifest_path.as_path(),
            self.logging_output_handler.clone(),
            params.cache_directory,
            config,
        )?;

        // Set the workspace dependencies
        instance.workspace_dependencies = workspace_dependencies;

        Ok((Box::new(instance), InitializeResult {}))
    }

    async fn negotiate_capabilities(
        _params: NegotiateCapabilitiesParams,
    ) -> miette::Result<NegotiateCapabilitiesResult> {
        Ok(NegotiateCapabilitiesResult {
            capabilities: default_capabilities(),
        })
    }
}

pub(crate) fn default_capabilities() -> BackendCapabilities {
    BackendCapabilities {
        provides_conda_outputs: Some(true),
        provides_conda_build_v1: Some(true),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pixi_build_backend::utils::test::conda_outputs_snapshot;
    use pixi_build_types::{VariantValue, procedures::initialize::InitializeParams};
    use rattler_build::console_utils::LoggingOutputHandler;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_conda_outputs() {
        insta::glob!("../../../tests/recipe", "*/recipe.yaml", |recipe_path| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let factory = RattlerBuildBackendInstantiator::new(LoggingOutputHandler::default())
                    .initialize(InitializeParams {
                        workspace_directory: None,
                        source_directory: None,
                        manifest_path: recipe_path.to_path_buf(),
                        project_model: None,
                        configuration: None,
                        target_configuration: None,
                        cache_directory: None,
                    })
                    .await
                    .unwrap();

                let current_dir = std::env::current_dir().unwrap();

                let result = factory
                    .0
                    .conda_outputs(CondaOutputsParams {
                        channels: vec![],
                        host_platform: Platform::Linux64,
                        build_platform: Platform::Linux64,
                        variant_configuration: None,
                        variant_files: None,
                        work_directory: current_dir,
                    })
                    .await
                    .unwrap();

                insta::assert_snapshot!(conda_outputs_snapshot(result));
            });
        });
    }

    const VARIANT_RECIPE: &str = r#"
    package:
      name: variant-test
      version: 0.1.0

    build:
      number: 0

    requirements:
      host:
        - python
        - numpy
    "#;

    #[tokio::test]
    async fn test_variant_files_are_applied() {
        let temp_dir = tempdir().unwrap();
        let recipe_path = temp_dir.path().join("recipe.yaml");
        tokio::fs::write(&recipe_path, VARIANT_RECIPE)
            .await
            .expect("Failed to write variant recipe");

        let variant_file = temp_dir.path().join("global-variants.yaml");
        tokio::fs::write(
            &variant_file,
            r#"python:
  - "3.9"
"#,
        )
        .await
        .expect("Failed to write variant file");

        let factory = RattlerBuildBackendInstantiator::new(LoggingOutputHandler::default())
            .initialize(InitializeParams {
                workspace_directory: None,
                source_directory: None,
                manifest_path: recipe_path,
                project_model: None,
                configuration: None,
                target_configuration: None,
                cache_directory: None,
            })
            .await
            .unwrap();

        let result = factory
            .0
            .conda_outputs(CondaOutputsParams {
                channels: vec![],
                host_platform: Platform::Linux64,
                build_platform: Platform::Linux64,
                variant_configuration: None,
                variant_files: Some(vec![variant_file.clone()]),
                work_directory: temp_dir.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert_eq!(result.outputs.len(), 1);
        let python_value = result.outputs[0]
            .metadata
            .variant
            .get("python")
            .expect("python variant present");
        assert_eq!(python_value, &VariantValue::from("3.9"));
    }

    #[tokio::test]
    async fn test_variant_configuration_is_applied() {
        let temp_dir = tempdir().unwrap();
        let recipe_path = temp_dir.path().join("recipe.yaml");
        tokio::fs::write(&recipe_path, VARIANT_RECIPE)
            .await
            .expect("Failed to write recipe");

        let mut variant_configuration = BTreeMap::new();
        variant_configuration.insert("python".to_string(), vec![VariantValue::from("3.11")]);

        let factory = RattlerBuildBackendInstantiator::new(LoggingOutputHandler::default())
            .initialize(InitializeParams {
                workspace_directory: None,
                source_directory: None,
                manifest_path: recipe_path.clone(),
                project_model: None,
                configuration: None,
                target_configuration: None,
                cache_directory: None,
            })
            .await
            .unwrap();

        let result = factory
            .0
            .conda_outputs(CondaOutputsParams {
                channels: vec![],
                host_platform: Platform::Linux64,
                build_platform: Platform::Linux64,
                variant_configuration: Some(variant_configuration),
                variant_files: None,
                work_directory: temp_dir.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert_eq!(
            result.outputs[0].metadata.variant["python"],
            VariantValue::from("3.11"),
            "Python variant should come from the provided configuration"
        );
        assert_eq!(
            result.outputs[0].metadata.variant["target_platform"],
            VariantValue::from("linux-64"),
            "Target platform should match the requested platform"
        );
    }

    #[tokio::test]
    async fn test_variant_configuration_overrides_variant_files() {
        let temp_dir = tempdir().unwrap();
        let recipe_path = temp_dir.path().join("recipe.yaml");
        tokio::fs::write(&recipe_path, VARIANT_RECIPE)
            .await
            .expect("Failed to write variant recipe");

        let variant_file = temp_dir.path().join("shared-variants.yaml");
        tokio::fs::write(
            &variant_file,
            r#"python:
  - "3.8"
numpy:
  - "1.22"
"#,
        )
        .await
        .expect("Failed to write shared variants");

        let mut variant_configuration = BTreeMap::new();
        variant_configuration.insert("python".to_string(), vec![VariantValue::from("3.10")]);

        let factory = RattlerBuildBackendInstantiator::new(LoggingOutputHandler::default())
            .initialize(InitializeParams {
                workspace_directory: None,
                source_directory: None,
                manifest_path: recipe_path,
                project_model: None,
                configuration: None,
                target_configuration: None,
                cache_directory: None,
            })
            .await
            .unwrap();

        let result = factory
            .0
            .conda_outputs(CondaOutputsParams {
                channels: vec![],
                host_platform: Platform::Linux64,
                build_platform: Platform::Linux64,
                variant_configuration: Some(variant_configuration),
                variant_files: Some(vec![variant_file.clone()]),
                work_directory: temp_dir.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert_eq!(result.outputs.len(), 1);
        let python_value = result.outputs[0]
            .metadata
            .variant
            .get("python")
            .expect("python variant present");
        assert_eq!(python_value, &VariantValue::from("3.10"));
        assert_eq!(
            result.outputs[0]
                .metadata
                .variant
                .get("numpy")
                .expect("numpy variant present from variant file"),
            &VariantValue::from("1.22")
        );
    }

    #[tokio::test]
    async fn test_variant_files_override_auto_discovered_variant() {
        let temp_dir = tempdir().unwrap();
        let recipe_path = temp_dir.path().join("recipe.yaml");
        tokio::fs::write(&recipe_path, VARIANT_RECIPE)
            .await
            .expect("Failed to write variant recipe");

        let auto_discovered_variant = temp_dir.path().join("variants.yaml");
        tokio::fs::write(
            &auto_discovered_variant,
            r#"python:
  - "3.8"
numpy:
  - "1.22"
"#,
        )
        .await
        .expect("Failed to write auto-discovered variants");

        let variant_file = temp_dir.path().join("override-variants.yaml");
        tokio::fs::write(
            &variant_file,
            r#"python:
  - "3.10"
"#,
        )
        .await
        .expect("Failed to write overriding variants");

        let factory = RattlerBuildBackendInstantiator::new(LoggingOutputHandler::default())
            .initialize(InitializeParams {
                workspace_directory: None,
                source_directory: None,
                manifest_path: recipe_path,
                project_model: None,
                configuration: None,
                target_configuration: None,
                cache_directory: None,
            })
            .await
            .unwrap();

        let result = factory
            .0
            .conda_outputs(CondaOutputsParams {
                channels: vec![],
                host_platform: Platform::Linux64,
                build_platform: Platform::Linux64,
                variant_configuration: None,
                variant_files: Some(vec![variant_file.clone()]),
                work_directory: temp_dir.path().to_path_buf(),
            })
            .await
            .unwrap();

        assert_eq!(result.outputs.len(), 1);
        let variant = &result.outputs[0].metadata.variant;
        assert_eq!(
            variant
                .get("python")
                .expect("python variant present after override"),
            &VariantValue::from("3.10")
        );
        assert_eq!(
            variant
                .get("numpy")
                .expect("numpy variant present from auto-discovered file"),
            &VariantValue::from("1.22")
        );
    }

    const FAKE_RECIPE: &str = r#"
    package:
      name: foobar
      version: 0.1.0
    "#;

    async fn try_initialize(
        manifest_path: impl AsRef<Path>,
    ) -> miette::Result<RattlerBuildBackend> {
        RattlerBuildBackend::new(
            None,
            manifest_path.as_ref(),
            LoggingOutputHandler::default(),
            None,
            RattlerBuildBackendConfig::default(),
        )
    }

    #[tokio::test]
    async fn test_recipe_discovery() {
        let tmp = tempdir().unwrap();
        let recipe = tmp.path().join("recipe.yaml");
        std::fs::write(&recipe, FAKE_RECIPE).unwrap();
        assert_eq!(
            try_initialize(&tmp.path().join("pixi.toml"))
                .await
                .unwrap()
                .recipe_source
                .path,
            recipe
        );
        assert_eq!(
            try_initialize(&recipe).await.unwrap().recipe_source.path,
            recipe
        );

        let tmp = tempdir().unwrap();
        let recipe = tmp.path().join("recipe.yml");
        std::fs::write(&recipe, FAKE_RECIPE).unwrap();
        assert_eq!(
            try_initialize(&tmp.path().join("pixi.toml"))
                .await
                .unwrap()
                .recipe_source
                .path,
            recipe
        );
        assert_eq!(
            try_initialize(&recipe).await.unwrap().recipe_source.path,
            recipe
        );

        let tmp = tempdir().unwrap();
        let recipe_dir = tmp.path().join("recipe");
        let recipe = recipe_dir.join("recipe.yaml");
        std::fs::create_dir(recipe_dir).unwrap();
        std::fs::write(&recipe, FAKE_RECIPE).unwrap();
        assert_eq!(
            try_initialize(&tmp.path().join("pixi.toml"))
                .await
                .unwrap()
                .recipe_source
                .path,
            recipe
        );

        let tmp = tempdir().unwrap();
        let recipe_dir = tmp.path().join("recipe");
        let recipe = recipe_dir.join("recipe.yml");
        std::fs::create_dir(recipe_dir).unwrap();
        std::fs::write(&recipe, FAKE_RECIPE).unwrap();
        assert_eq!(
            try_initialize(&tmp.path().join("pixi.toml"))
                .await
                .unwrap()
                .recipe_source
                .path,
            recipe
        );
    }

    #[test]
    fn test_relative_path_joined() {
        use std::path::Path;
        // Simple case
        let base = Path::new("/foo/bar");
        let input = Path::new("/foo/bar/baz/qux.txt");
        assert_eq!(
            super::build_relative_glob(base, input).unwrap(),
            "baz/qux.txt"
        );
        // Same path
        let base = Path::new("/foo/bar");
        let input = Path::new("/foo/bar");
        assert_eq!(super::build_relative_glob(base, input).unwrap(), "");
        // Input not under base
        let base = Path::new("/foo/bar");
        let input = Path::new("/foo/other");
        assert_eq!(super::build_relative_glob(base, input).unwrap(), "../other");
        // Relative paths
        let base = Path::new("foo/bar");
        let input = Path::new("foo/bar/baz");
        assert_eq!(super::build_relative_glob(base, input).unwrap(), "baz");
    }

    #[test]
    #[cfg(windows)]
    fn test_relative_path_joined_windows() {
        use std::path::Path;
        let base = Path::new(r"C:\foo\bar");
        let input = Path::new(r"C:\foo\bar\baz\qux.txt");
        assert_eq!(
            super::build_relative_glob(base, input).unwrap(),
            "baz/qux.txt"
        );
        let base = Path::new(r"C:\foo\bar");
        let input = Path::new(r"C:\foo\bar");
        assert_eq!(super::build_relative_glob(base, input).unwrap(), "");
        let base = Path::new(r"C:\foo\bar");
        let input = Path::new(r"C:\foo\other");
        assert_eq!(super::build_relative_glob(base, input).unwrap(), "../other");
    }

    #[test]
    fn test_build_input_globs_with_tempdirs() {
        use std::fs;

        use tempfile::tempdir;

        // Create a temp directory to act as the base
        let base_dir = tempdir().unwrap();
        let base_path = base_dir.path();

        // Case 1: source is a file in the base dir
        let recipe_path = base_path.join("recipe.yaml");
        fs::write(&recipe_path, "fake").unwrap();
        let globs = super::build_input_globs(base_path, &recipe_path, None, Vec::new()).unwrap();
        assert_eq!(globs, BTreeSet::from([String::from("**")]));

        // Case 2: source is a directory, with a file and a dir as package sources
        let pkg_dir = base_path.join("pkg");
        let pkg_file = pkg_dir.join("file.txt");
        let pkg_subdir = pkg_dir.join("dir");
        fs::create_dir_all(&pkg_subdir).unwrap();
        fs::write(&pkg_file, "fake").unwrap();
        let globs = super::build_input_globs(
            base_path,
            base_path,
            Some(vec![pkg_file.clone(), pkg_subdir.clone()]),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            globs,
            BTreeSet::from([
                String::from("**"),
                String::from("pkg/file.txt"),
                String::from("pkg/dir/**")
            ])
        );
    }

    #[test]
    fn test_build_input_globs_two_folders_in_tempdir() {
        use std::fs;

        use tempfile::tempdir;

        // Create a temp directory
        let temp = tempdir().unwrap();
        let temp_path = temp.path();

        // Create two folders: source_dir and package_source_dir
        let source_dir = temp_path.join("source");
        let package_source_dir = temp_path.join("pkgsrc");
        fs::create_dir_all(&source_dir).unwrap();
        fs::create_dir_all(&package_source_dir).unwrap();

        // Call build_input_globs with source_dir as source, and package_source_dir as
        // package source
        let globs = super::build_input_globs(
            temp_path,
            &source_dir,
            Some(vec![package_source_dir.clone()]),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(
            globs,
            BTreeSet::from([String::from("source/**"), String::from("pkgsrc/**")])
        );
    }

    #[test]
    fn test_build_input_globs_relative_source() {
        use std::{fs, path::PathBuf};

        use tempfile::tempdir;

        // Create a temp directory to act as the base
        let base_dir = tempdir().unwrap();
        let base_path = base_dir.path();

        // Case: source is a directory, package_sources contains a relative path
        let rel_dir = PathBuf::from("rel_folder");
        let abs_rel_dir = base_path.join(&rel_dir);
        fs::create_dir_all(&abs_rel_dir).unwrap();

        // Call build_input_globs with base_path as source, and rel_dir as package
        // source (relative)
        let globs = super::build_input_globs(
            base_path,
            base_path,
            Some(vec![rel_dir.clone()]),
            Vec::new(),
        )
        .unwrap();
        // The relative path from base_path to rel_dir should be "rel_folder/**"
        assert_eq!(
            globs,
            BTreeSet::from_iter(["**", "rel_folder/**"].into_iter().map(ToString::to_string))
        );
    }

    #[test]
    fn test_get_metadata_input_globs() {
        use std::path::PathBuf;
        // Case: file with name
        let manifest_root = PathBuf::from("/foo/bar");
        let path = PathBuf::from("/foo/bar/recipe.yaml");
        let globs = super::get_metadata_input_globs(&manifest_root, &path).unwrap();
        assert_eq!(globs, BTreeSet::from([String::from("recipe.yaml")]));
        // Case: file with no name (root)
        let manifest_root = PathBuf::from("/");
        let path = PathBuf::from("/");
        let globs = super::get_metadata_input_globs(&manifest_root, &path).unwrap();
        assert_eq!(globs, BTreeSet::from([String::from("**")]));
        // Case: file with .yml extension
        let manifest_root = PathBuf::from("/foo/bar");
        let path = PathBuf::from("/foo/bar/recipe.yml");
        let globs = super::get_metadata_input_globs(&manifest_root, &path).unwrap();
        assert_eq!(globs, BTreeSet::from([String::from("recipe.yml")]));
        // Case: file in subdir
        let manifest_root = PathBuf::from("/foo");
        let path = PathBuf::from("/foo/bar/recipe.yaml");
        let globs = super::get_metadata_input_globs(&manifest_root, &path).unwrap();
        assert_eq!(globs, BTreeSet::from([String::from("bar/recipe.yaml")]));
    }

    #[test]
    fn test_build_input_globs_includes_extra_globs() {
        use std::fs;

        use tempfile::tempdir;

        // Create a temp directory to act as the base
        let base_dir = tempdir().unwrap();
        let base_path = base_dir.path();

        // Create a recipe file
        let recipe_path = base_path.join("recipe.yaml");
        fs::write(&recipe_path, "fake").unwrap();

        // Test with extra globs
        let extra_globs = vec!["custom/*.txt".to_string(), "extra/**/*.py".to_string()];
        let globs =
            super::build_input_globs(base_path, &recipe_path, None, extra_globs.clone()).unwrap();

        // Verify that all extra globs are included in the result
        for extra_glob in &extra_globs {
            assert!(
                globs.contains(extra_glob),
                "Result should contain extra glob: {extra_glob}"
            );
        }

        // Verify that the basic manifest glob is still present
        assert!(globs.contains("**"));
    }
}
