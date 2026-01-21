use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use indexmap::{IndexMap, IndexSet};
use itertools::Itertools;
use miette::{Context, IntoDiagnostic};
use ordermap::OrderMap;
use pixi_build_types::{
    BackendCapabilities, PathSpec, ProjectModel, SourcePackageSpec, TargetSelector,
    procedures::{
        conda_build_v1::{CondaBuildV1Output, CondaBuildV1Params, CondaBuildV1Result},
        conda_outputs::{
            CondaOutput, CondaOutputDependencies, CondaOutputIgnoreRunExports, CondaOutputMetadata,
            CondaOutputRunExports, CondaOutputsParams, CondaOutputsResult,
        },
        initialize::{InitializeParams, InitializeResult},
        negotiate_capabilities::{NegotiateCapabilitiesParams, NegotiateCapabilitiesResult},
    },
};
use rattler_build::{
    NormalizedKey,
    build::{WorkingDirectoryBehavior, run_build},
    console_utils::LoggingOutputHandler,
    hash::HashInfo,
    metadata::{BuildConfiguration, Debug, Output, PlatformWithVirtualPackages},
    recipe::{ParsingError, Recipe, parser::find_outputs_from_src},
    selectors::SelectorConfig,
    source_code::Source,
    tool_configuration::Configuration,
    types::{Directories, PackageIdentifier, PackagingSettings},
    variant_config::{DiscoveredOutput, ParseErrors, VariantConfig},
};
use rattler_conda_types::{Platform, compression_level::CompressionLevel, package::ArchiveType};

use serde::Deserialize;
use tracing::warn;

use crate::{
    consts::DEBUG_OUTPUT_DIR,
    dependencies::{
        convert_constraint_dependencies, convert_dependencies, convert_input_variant_configuration,
    },
    generated_recipe::{BackendConfig, GenerateRecipe, PythonParams},
    protocol::{Protocol, ProtocolInstantiator},
    specs_conversion::{
        convert_variant_from_pixi_build_types, convert_variant_to_pixi_build_types,
        from_build_v1_args_to_finalized_dependencies,
    },
    tools::{OneOrMultipleOutputs, output_directory},
    traits::targets::TargetSelector as _,
};

use fs_err::tokio as tokio_fs;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IntermediateBackendConfig {
    /// Environment Variables
    #[serde(default)]
    pub env: IndexMap<String, String>,
    /// If set, internal state will be logged as files in that directory
    pub debug_dir: Option<PathBuf>,
}

pub struct IntermediateBackendInstantiator<T: GenerateRecipe> {
    logging_output_handler: LoggingOutputHandler,

    generator: Arc<T>,
}

impl<T: GenerateRecipe> IntermediateBackendInstantiator<T> {
    pub fn new(logging_output_handler: LoggingOutputHandler, instance: Arc<T>) -> Self {
        Self {
            logging_output_handler,
            generator: instance,
        }
    }
}

pub struct IntermediateBackend<T: GenerateRecipe> {
    pub(crate) logging_output_handler: LoggingOutputHandler,
    pub(crate) source_dir: PathBuf,
    /// The path to the manifest file relative to the source directory.
    pub(crate) manifest_rel_path: PathBuf,
    pub(crate) project_model: ProjectModel,
    pub(crate) generate_recipe: Arc<T>,
    pub(crate) config: T::Config,
    pub(crate) target_config: OrderMap<TargetSelector, T::Config>,
    pub(crate) cache_dir: Option<PathBuf>,
}
impl<T: GenerateRecipe> IntermediateBackend<T> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        manifest_path: PathBuf,
        source_dir: Option<PathBuf>,
        project_model: ProjectModel,
        generate_recipe: Arc<T>,
        config: serde_json::Value,
        target_config: OrderMap<TargetSelector, serde_json::Value>,
        logging_output_handler: LoggingOutputHandler,
        cache_dir: Option<PathBuf>,
    ) -> miette::Result<Self> {
        // Determine the root directory of the manifest
        let (source_dir, manifest_rel_path) = match source_dir {
            None => {
                let source_dir = manifest_path
                    .parent()
                    .ok_or_else(|| {
                        miette::miette!("the project manifest must reside in a directory")
                    })?
                    .to_path_buf();
                let manifest_rel_path = manifest_path
                    .file_name()
                    .map(Path::new)
                    .expect("we already validated that the manifest path is a file")
                    .to_path_buf();
                (source_dir, manifest_rel_path)
            }
            Some(source_dir) => {
                let manifest_rel_path = pathdiff::diff_paths(&manifest_path, &source_dir)
                    .ok_or_else(|| {
                        miette::miette!(
                            "the manifest: {} is not relative to the source directory: {}",
                            manifest_path.display(),
                            source_dir.display()
                        )
                    })?;
                (source_dir, manifest_rel_path)
            }
        };

        let config = serde_json::from_value::<T::Config>(config)
            .into_diagnostic()
            .context("failed to parse configuration")?;

        if let Some(path) = config.debug_dir() {
            warn!(
                path = %path.display(),
                "`debug-dir` backend configuration is deprecated and ignored; debug data is now written to the build work directory."
            );
        }

        let target_config = target_config
            .into_iter()
            .map(|(target, config)| {
                let config = serde_json::from_value::<T::Config>(config)
                    .into_diagnostic()
                    .wrap_err_with(|| {
                        format!("failed to parse target configuration for {target}")
                    })?;
                Ok((target, config))
            })
            .collect::<Result<_, miette::Report>>()?;

        Ok(Self {
            source_dir,
            manifest_rel_path,
            project_model,
            generate_recipe,
            config,
            target_config,
            logging_output_handler,
            cache_dir,
        })
    }
}

#[async_trait::async_trait]
impl<T> ProtocolInstantiator for IntermediateBackendInstantiator<T>
where
    T: GenerateRecipe + Clone + Send + Sync + 'static,
    T::Config: Send + Sync + 'static,
{
    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> miette::Result<(Box<dyn Protocol + Send + Sync + 'static>, InitializeResult)> {
        let project_model = params
            .project_model
            .ok_or_else(|| miette::miette!("project model is required"))?;

        let config = if let Some(config) = params.configuration {
            config
        } else {
            serde_json::Value::Object(Default::default())
        };

        let target_config = params.target_configuration.unwrap_or_default();

        let instance = IntermediateBackend::<T>::new(
            params.manifest_path,
            params.source_directory,
            project_model,
            self.generator.clone(),
            config,
            target_config,
            self.logging_output_handler.clone(),
            params.cache_directory,
        )?;

        Ok((Box::new(instance), InitializeResult {}))
    }

    async fn negotiate_capabilities(
        _params: NegotiateCapabilitiesParams,
    ) -> miette::Result<NegotiateCapabilitiesResult> {
        // Returns the capabilities of this backend based on the capabilities of
        // the frontend.
        Ok(NegotiateCapabilitiesResult {
            capabilities: default_capabilities(),
        })
    }
}

#[async_trait::async_trait]
impl<T> Protocol for IntermediateBackend<T>
where
    T: GenerateRecipe + Clone + Send + Sync + 'static,
    T::Config: BackendConfig + Send + Sync + 'static,
{
    async fn conda_outputs(
        &self,
        params: CondaOutputsParams,
    ) -> miette::Result<CondaOutputsResult> {
        let build_platform = params.host_platform;

        let config = self
            .target_config
            .iter()
            .find(|(selector, _)| selector.matches(params.host_platform))
            .map(|(_, target_config)| self.config.merge_with_target_config(target_config))
            .unwrap_or_else(|| Ok(self.config.clone()))?;

        let selector_config_for_variants = SelectorConfig {
            target_platform: params.host_platform,
            host_platform: params.host_platform,
            build_platform,
            hash: None,
            variant: Default::default(),
            experimental: false,
            allow_undefined: false,
            recipe_path: Some(self.source_dir.join(&self.manifest_rel_path)),
        };

        let mut variants = self
            .generate_recipe
            .default_variants(params.host_platform)?;

        // Construct a `VariantConfig` based on the input parameters. This is a
        // combination of defaults provided by the generator (lowest priority),
        // variants loaded from external files, and finally the user supplied
        // variants (highest priority).
        let variant_files = params.variant_files.unwrap_or_default();
        let mut variant_config =
            VariantConfig::from_files(&variant_files, &selector_config_for_variants)?;
        variants.append(&mut variant_config.variants);
        variant_config.variants = variants;

        let mut param_variants =
            convert_input_variant_configuration(params.variant_configuration.clone())
                .unwrap_or_default();
        variant_config.variants.append(&mut param_variants);

        // Construct the intermediate recipe
        let generated_recipe = self
            .generate_recipe
            .generate_recipe(
                &self.project_model,
                &config,
                self.source_dir.clone(),
                params.host_platform,
                Some(PythonParams { editable: false }),
                &variant_config.variants.keys().cloned().collect(),
                params.channels,
                self.cache_dir.clone(),
            )
            .await?;

        // Convert the recipe to source code.
        // TODO(baszalmstra): In the future it would be great if we could just
        // immediately use the intermediate recipe for some of this rattler-build
        // functions.
        let recipe_path = self.source_dir.join(&self.manifest_rel_path);
        let named_source = Source {
            name: self.manifest_rel_path.display().to_string(),
            code: Arc::from(
                generated_recipe
                    .recipe
                    .to_yaml_pretty()
                    .into_diagnostic()?
                    .as_str(),
            ),
            path: recipe_path.clone(),
        };

        // Determine the different outputs that are supported by the recipe by expanding
        // all the different variant combinations.
        //
        // TODO(baszalmstra): The selector config we pass in here doesnt have all values
        // filled in. This is on purpose because at this point we dont yet know all
        // values like the variant. We should introduce a new type of selector config
        // for this particular case.
        let outputs = find_outputs_from_src(named_source.clone())?;
        let discovered_outputs = variant_config.find_variants(
            &outputs,
            named_source.clone(),
            &selector_config_for_variants,
        )?;

        // Construct a mapping that for packages that we want from source.
        //
        // By default, this includes all the outputs in the recipe. These should all be
        // build from source, in particular from the current source.
        let local_source_packages: HashMap<String, SourcePackageSpec> = discovered_outputs
            .iter()
            .map(|output| (output.name.clone(), PathSpec { path: ".".into() }.into()))
            .collect();

        let mut subpackages = HashMap::new();
        let mut outputs = Vec::new();

        let num_of_outputs = discovered_outputs.len();

        let mut variants_saved = false;

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
                        .map(|err| ParsingError::from_partial(named_source.clone(), err))
                        .collect::<Vec<_>>()
                        .into();
                    errs
                })?;

            // Skip this output if the recipe is marked as skipped
            if recipe.build().skip() {
                continue;
            }

            let build_number = recipe.build().number;

            let directories = output_directory(
                if num_of_outputs == 1 {
                    OneOrMultipleOutputs::Single(discovered_output.name.clone())
                } else {
                    OneOrMultipleOutputs::OneOfMany(discovered_output.name.clone())
                },
                params.work_directory.clone(),
                &named_source.path,
            );

            // Save intermediate recipe and the used variant
            // in the debug dir by hash of the variant
            // and entire variants.yaml at the root of debug_dir
            let debug_dir = &directories.build_dir.join("debug");

            let recipe_path = debug_dir.join("recipe.yaml");
            let variants_path = debug_dir.join("variants.yaml");

            let package_debug_dir = debug_dir.join("recipe").join(&hash.hash);
            let package_recipe_path = package_debug_dir.join("recipe.yaml");
            let package_variant_path = package_debug_dir.join("variants.yaml");

            tokio_fs::create_dir_all(&package_debug_dir)
                .await
                .into_diagnostic()?;

            let recipe_yaml = generated_recipe.recipe.to_yaml_pretty().into_diagnostic()?;

            tokio_fs::write(&package_recipe_path, &recipe_yaml)
                .await
                .into_diagnostic()?;

            let variant_yaml = serde_yaml::to_string(&variant)
                .into_diagnostic()
                .context("failed to serialize variant to YAML")?;

            tokio_fs::write(&package_variant_path, variant_yaml)
                .await
                .into_diagnostic()?;

            // write the entire variants.yaml at the root of debug_dir
            if !variants_saved {
                let variants = serde_yaml::to_string(&variant_config)
                    .into_diagnostic()
                    .context("failed to serialize variant config to YAML")?;

                tokio_fs::write(&variants_path, variants)
                    .await
                    .into_diagnostic()?;

                tokio_fs::write(&recipe_path, recipe_yaml)
                    .await
                    .into_diagnostic()?;

                variants_saved = true;
            }

            subpackages.insert(
                recipe.package().name().clone(),
                PackageIdentifier {
                    name: recipe.package().name().clone(),
                    version: recipe.package().version().clone(),
                    build_string: discovered_output.build_string.clone(),
                },
            );

            outputs.push(CondaOutput {
                metadata: CondaOutputMetadata {
                    name: recipe.package().name().clone(),
                    version: recipe.package.version().clone(),
                    build: discovered_output.build_string.clone(),
                    build_number,
                    subdir: discovered_output.target_platform,
                    license: recipe.about.license.map(|l| l.to_string()),
                    license_family: recipe.about.license_family,
                    noarch: recipe.build.noarch,
                    purls: None,
                    python_site_packages_path: None,
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

        Ok(CondaOutputsResult {
            outputs,
            input_globs: generated_recipe.metadata_input_globs,
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

        let config = self
            .target_config
            .iter()
            .find(|(selector, _)| selector.matches(host_platform))
            .map(|(_, target_config)| self.config.merge_with_target_config(target_config))
            .unwrap_or_else(|| Ok(self.config.clone()))?;

        // Construct the variants based on the input parameters. We only
        // have a single variant here so we can just use the variant from the
        // parameters.
        let variants: BTreeMap<_, _> = params
            .output
            .variant
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().into(),
                    vec![convert_variant_from_pixi_build_types(v.clone())],
                )
            })
            .collect();

        // Construct the intermediate recipe
        let mut recipe = self
            .generate_recipe
            .generate_recipe(
                &self.project_model,
                &config,
                self.source_dir.clone(),
                host_platform,
                Some(PythonParams {
                    editable: params.editable.unwrap_or_default(),
                }),
                &variants.keys().cloned().collect(),
                params.channels,
                self.cache_dir.clone(),
            )
            .await?;

        // Convert the recipe to source code.
        // TODO(baszalmstra): In the future it would be great if we could just
        // immediately use the intermediate recipe for some of this rattler-build
        // functions.
        let recipe_path = self.source_dir.join(&self.manifest_rel_path);
        let named_source = Source {
            name: self.manifest_rel_path.display().to_string(),
            code: Arc::from(recipe.recipe.to_yaml_pretty().into_diagnostic()?.as_str()),
            path: recipe_path.clone(),
        };

        // Determine the different outputs that are supported by the recipe.
        let selector_config_for_variants = SelectorConfig {
            target_platform: host_platform,
            host_platform,
            build_platform,
            hash: None,
            variant: Default::default(),
            experimental: false,
            allow_undefined: false,
            recipe_path: Some(self.source_dir.join(&self.manifest_rel_path)),
        };
        let outputs = find_outputs_from_src(named_source.clone())?;

        let variant_config = VariantConfig {
            variants,
            pin_run_as_build: None,
            zip_keys: None,
        };
        let discovered_outputs = variant_config.find_variants(
            &outputs,
            named_source.clone(),
            &selector_config_for_variants,
        )?;
        let discovered_output = find_matching_output(&params.output, discovered_outputs)?;

        // Set up the proper directories for the build.
        let directories = conda_build_v1_directories(
            params.host_prefix.as_ref().map(|p| p.prefix.as_path()),
            params.build_prefix.as_ref().map(|p| p.prefix.as_path()),
            params.work_directory.clone(),
            self.cache_dir.as_deref(),
            params.output_directory.as_deref(),
            recipe_path,
        );

        // Save intermediate recipe and the used variant
        // in the debug dir by hash of the variant
        let variant = discovered_output.used_vars;

        // Save intermediate recipe and the used variant
        // in the debug dir by hash of the variant
        // and entire variants.yaml at the root of debug_dir
        let debug_dir = &directories.build_dir.join(DEBUG_OUTPUT_DIR);

        let recipe_path = debug_dir.join("recipe.yaml");
        let variants_path = debug_dir.join("variants.yaml");

        let package_dir = debug_dir.join("recipe").join(&discovered_output.hash.hash);

        let package_recipe_path = package_dir.join("recipe.yaml");
        let package_variant_path = package_dir.join("variants.yaml");

        tokio_fs::create_dir_all(&package_dir)
            .await
            .into_diagnostic()?;

        let recipe_yaml = recipe.recipe.to_yaml_pretty().into_diagnostic()?;

        tokio_fs::write(&package_recipe_path, &recipe_yaml)
            .await
            .into_diagnostic()?;

        let variant_yaml = serde_yaml::to_string(&variant)
            .into_diagnostic()
            .context("failed to serialize variant to YAML")?;

        tokio_fs::write(&package_variant_path, variant_yaml)
            .await
            .into_diagnostic()?;

        // write the entire variants.yaml at the root of debug_dir
        let variants = serde_yaml::to_string(&variant_config)
            .into_diagnostic()
            .context("failed to serialize variant config to YAML")?;

        tokio_fs::write(&variants_path, variants)
            .await
            .into_diagnostic()?;

        tokio_fs::write(&recipe_path, recipe_yaml)
            .await
            .into_diagnostic()?;

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
                variant,
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
            run_build(output, &tool_config, WorkingDirectoryBehavior::Preserve).await?;

        // Extract the input globs from the build and recipe
        let mut input_globs = self.generate_recipe.extract_input_globs_from_build(
            &config,
            &params.work_directory,
            params.editable.unwrap_or_default(),
        )?;
        input_globs.append(&mut recipe.build_input_globs);

        Ok(CondaBuildV1Result {
            output_file: output_path,
            input_globs,
            name: output.name().as_normalized().to_string(),
            version: output.version().clone(),
            build: output.build_string().into_owned(),
            subdir: *output.target_platform(),
        })
    }
}

pub fn find_matching_output(
    expected_output: &CondaBuildV1Output,
    mut discovered_outputs: IndexSet<DiscoveredOutput>,
) -> miette::Result<DiscoveredOutput> {
    // Filter all outputs that are skipped or dont match the name
    discovered_outputs.retain(|output| {
        !output.recipe.build.skip() && output.name == expected_output.name.as_normalized()
    });

    if discovered_outputs.is_empty() {
        // There is no output with the expected name
        return Err(miette::miette!(
            "there is no output defined for the package '{}'",
            expected_output.name.as_source(),
        ));
    }

    // Check if there is a output that has matching variant keys.
    let expected_used_vars: BTreeMap<NormalizedKey, _> = expected_output
        .variant
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().into(),
                convert_variant_from_pixi_build_types(v.clone()),
            )
        })
        .collect();
    if let Ok((output_idx, _)) = discovered_outputs
        .iter()
        .enumerate()
        .filter(|(_idx, output)| {
            expected_used_vars
                .iter()
                .all(|(key, value)| output.used_vars.get(key) == Some(value))
        })
        .exactly_one()
    {
        return Ok(discovered_outputs
            .swap_remove_index(output_idx)
            .expect("index must exist"));
    }

    // Otherwise, match on version, build and subdir.
    discovered_outputs
        .into_iter()
        .find(|output| {
            expected_output
                .build
                .as_ref()
                .is_none_or(|build_string| build_string == &output.build_string)
                && expected_output
                    .version
                    .as_ref()
                    .is_none_or(|version| version == &output.recipe.package.version)
                && expected_output.subdir == output.target_platform
        })
        .ok_or_else(|| {
            miette::miette!(
                "the requested output {}/{}={}@{} was not found in the recipe",
                expected_output.name.as_source(),
                expected_output
                    .version
                    .as_ref()
                    .map_or_else(|| String::from("??"), |v| v.as_str().into_owned()),
                expected_output.build.as_deref().unwrap_or("??"),
                expected_output.subdir
            )
        })
}

pub fn conda_build_v1_directories(
    host_prefix: Option<&Path>,
    build_prefix: Option<&Path>,
    work_directory: PathBuf,
    cache_dir: Option<&Path>,
    output_dir: Option<&Path>,
    recipe_path: PathBuf,
) -> Directories {
    Directories {
        recipe_dir: recipe_path
            .parent()
            .expect("recipe path must have a parent")
            .to_path_buf(),
        recipe_path,
        cache_dir: cache_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| work_directory.join("cache")),
        host_prefix: host_prefix
            .map(Path::to_path_buf)
            .unwrap_or_else(|| work_directory.join("host")),
        build_prefix: build_prefix
            .map(Path::to_path_buf)
            .unwrap_or_else(|| work_directory.join("build")),
        work_dir: work_directory.join("work"),
        output_dir: output_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| work_directory.join("output")),
        build_dir: work_directory,
    }
}

/// Returns the capabilities for this backend
fn default_capabilities() -> BackendCapabilities {
    BackendCapabilities {
        provides_conda_outputs: Some(true),
        provides_conda_build_v1: Some(true),
    }
}
