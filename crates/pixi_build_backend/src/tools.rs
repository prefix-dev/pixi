use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use indexmap::IndexSet;
use miette::IntoDiagnostic;
use rattler_build::{
    DiscoveredOutput,
    metadata::{BuildConfiguration, Debug, Output, PlatformWithVirtualPackages},
    system_tools::SystemTools,
    types::{Directories, PackageIdentifier, PackagingSettings},
};
use rattler_build_recipe::{
    stage1::{GlobVec, Source as RecipeSource},
    variant_render::RenderConfig,
};
use rattler_build_variant_config::{VariantConfig, VariantConfigError};
use rattler_conda_types::compression_level::CompressionLevel;
use rattler_conda_types::{GenericVirtualPackage, NoArchType, Platform, package::CondaArchiveType};
use rattler_virtual_packages::VirtualPackageOverrides;
use url::Url;

use crate::{source::Source, specs_conversion::convert_variant_from_pixi_build_types};

/// A `recipe.yaml` file might be accompanied by a `variants.toml` file from
/// which we can read variant configuration for that specific recipe..
pub const VARIANTS_CONFIG_FILE: &str = "variants.yaml";

/// A struct that contains all the configuration needed
/// for `rattler-build` in order to build a recipe.
/// The principal concepts is that all rattler-build concepts
/// should be hidden behind this struct and all pixi-build-backends
/// should only interact with this struct.
pub struct RattlerBuild {
    /// The source of the recipe
    pub recipe_source: Source,
    /// The target platform for the build.
    pub target_platform: Platform,
    /// The host platform for the build.
    pub host_platform: Platform,
    /// The build platform for the build.
    pub build_platform: Platform,
    /// Whether experimental features are enabled.
    pub experimental: bool,
    /// The directory where the build should happen.
    pub work_directory: PathBuf,
}

/// Variant configuration that was loaded from the recipe.
#[derive(Debug)]
pub struct LoadedVariantConfig {
    /// The variant configuration that was loaded.
    pub variant_config: VariantConfig,

    /// Input globs that identity the files that were loaded.
    pub input_globs: BTreeSet<String>,
}

impl LoadedVariantConfig {
    /// Load variant configuration from a recipe path. This checks if there is a
    /// `variants.yaml` and loads it alongside the recipe.
    #[allow(clippy::result_large_err)]
    pub fn from_recipe_path<'a>(
        source_dir: &Path,
        recipe_path: &Path,
        target_platform: Platform,
        additional_variant_files: impl Iterator<Item = &'a Path>,
    ) -> Result<Self, VariantConfigError> {
        let mut variant_files = Vec::new();
        let mut input_globs = BTreeSet::new();

        // Check if there is a `variants.yaml` file next to the recipe that we should
        // potentially use.
        if let Some(variant_path) = recipe_path
            .parent()
            .map(|parent| parent.join(VARIANTS_CONFIG_FILE))
        {
            if let Some(path) = pathdiff::diff_paths(&variant_path, source_dir) {
                // Normalize paths on windows
                let normalized_path = if cfg!(target_os = "windows") {
                    path.to_string_lossy().replace("\\", "/")
                } else {
                    path.to_string_lossy().to_string()
                };
                input_globs.insert(normalized_path);
            }
            if variant_path.is_file() {
                variant_files.push(variant_path);
            }
        };

        // Add additional variant files
        variant_files.extend(additional_variant_files.map(|p| p.to_path_buf()));

        Ok(Self {
            variant_config: VariantConfig::from_files(&variant_files, target_platform)?,
            input_globs,
        })
    }

    pub fn extend_with_input_variants(
        mut self,
        input_variant_configuration: BTreeMap<String, Vec<pixi_build_types::VariantValue>>,
    ) -> Self {
        for (k, v) in input_variant_configuration {
            let variables = v
                .into_iter()
                .map(convert_variant_from_pixi_build_types)
                .collect();
            self.variant_config.variants.insert(k.into(), variables);
        }
        self
    }
}

pub enum OneOrMultipleOutputs {
    Single(String),
    OneOfMany(String),
}

impl RattlerBuild {
    /// Create a new `RattlerBuild` instance.
    pub fn new(
        source: Source,
        target_platform: Platform,
        host_platform: Platform,
        build_platform: Platform,
        experimental: bool,
        work_directory: PathBuf,
    ) -> Self {
        Self {
            recipe_source: source,
            target_platform,
            host_platform,
            build_platform,
            experimental,
            work_directory,
        }
    }

    /// Discover the outputs from the recipe.
    pub fn discover_outputs(
        &self,
        variant_files: &[PathBuf],
        variant_config_input: &Option<BTreeMap<String, Vec<String>>>,
    ) -> miette::Result<IndexSet<DiscoveredOutput>> {
        // Create source for error reporting
        let source = rattler_build_recipe::source_code::Source::from_string(
            self.recipe_source.name.clone(),
            self.recipe_source.code.to_string(),
        );

        // Parse the recipe into a stage0 representation
        let stage0_recipe = rattler_build_recipe::parse_recipe(&source)?;

        // Check if there is a `variants.yaml` file next to the recipe that we should
        // potentially use.
        let mut variant_config_paths = Vec::new();
        if let Some(variant_path) = self
            .recipe_source
            .path
            .parent()
            .map(|parent| parent.join(VARIANTS_CONFIG_FILE))
            && variant_path.is_file()
        {
            variant_config_paths.push(variant_path);
        };

        variant_config_paths.extend(variant_files.iter().cloned());

        let mut variant_config =
            VariantConfig::from_files(&variant_config_paths, self.target_platform)
                .into_diagnostic()?;

        if let Some(variant_config_input) = variant_config_input {
            for (k, v) in variant_config_input.iter() {
                let variables = v.iter().map(|v| v.clone().into()).collect();
                variant_config.variants.insert(k.as_str().into(), variables);
            }
        }

        // Build render config
        let render_config = RenderConfig::new()
            .with_target_platform(self.target_platform)
            .with_build_platform(self.build_platform)
            .with_host_platform(self.host_platform)
            .with_experimental(self.experimental)
            .with_recipe_path(&self.recipe_source.path);

        // Render recipe with variant config
        let rendered_variants = rattler_build_recipe::render_recipe(
            &source,
            &stage0_recipe,
            &variant_config,
            render_config,
        )?;

        // Convert RenderedVariants to DiscoveredOutputs
        let mut outputs = IndexSet::new();
        for rendered in rendered_variants {
            let recipe = rendered.recipe;
            let variant = rendered.variant;

            let effective_target_platform = if recipe.build().noarch.is_none() {
                self.target_platform
            } else {
                Platform::NoArch
            };

            let build_string = recipe
                .build()
                .string
                .as_resolved()
                .expect("build string should be resolved after evaluation")
                .to_string();

            outputs.insert(DiscoveredOutput {
                name: recipe.package().name().as_normalized().to_string(),
                version: recipe.package().version().to_string(),
                build_string,
                noarch_type: recipe.build().noarch.unwrap_or(NoArchType::none()),
                target_platform: effective_target_platform,
                used_vars: variant,
                recipe,
                hash: rendered
                    .hash_info
                    .expect("hash should be set after evaluation"),
            });
        }

        Ok(outputs)
    }

    /// Get the outputs from the recipe.
    pub fn get_outputs(
        &self,
        discovered_outputs: &IndexSet<DiscoveredOutput>,
        channels: Vec<Url>,
        build_vpkgs: Vec<GenericVirtualPackage>,
        host_vpkgs: Vec<GenericVirtualPackage>,
        host_platform: Platform,
        build_platform: Platform,
    ) -> miette::Result<Vec<Output>> {
        let mut outputs = Vec::new();

        let mut subpackages = BTreeMap::new();

        let channels: Vec<_> = channels.into_iter().map(Into::into).collect();
        for discovered_output in discovered_outputs {
            let mut recipe = discovered_output.recipe.clone();

            // Add .pixi to the exclude list for path sources
            for source in &mut recipe.source {
                if let RecipeSource::Path(path_source) = source {
                    let include = path_source
                        .filter
                        .include_globs()
                        .iter()
                        .map(|g| g.source())
                        .collect();
                    let exclude = path_source
                        .filter
                        .exclude_globs()
                        .iter()
                        .map(|g| g.source())
                        .chain([".pixi"])
                        .collect();
                    path_source.filter = GlobVec::from_vec(include, Some(exclude));
                }
            }

            if recipe.build().skip {
                eprintln!(
                    "Skipping build for variant: {:#?}",
                    discovered_output.used_vars
                );
                continue;
            }

            subpackages.insert(
                recipe.package().name().clone(),
                PackageIdentifier {
                    name: recipe.package().name().clone(),
                    version: recipe.package().version().clone(),
                    build_string: discovered_output.build_string.clone(),
                },
            );

            outputs.push(Output {
                recipe,
                build_configuration: BuildConfiguration {
                    target_platform: discovered_output.target_platform,
                    host_platform: PlatformWithVirtualPackages {
                        platform: host_platform,
                        virtual_packages: host_vpkgs.clone(),
                    },
                    build_platform: PlatformWithVirtualPackages {
                        platform: build_platform,
                        virtual_packages: build_vpkgs.clone(),
                    },
                    hash: discovered_output.hash.clone(),
                    variant: discovered_output.used_vars.clone(),
                    directories: output_directory(
                        if discovered_outputs.len() == 1 {
                            OneOrMultipleOutputs::Single(discovered_output.name.clone())
                        } else {
                            OneOrMultipleOutputs::OneOfMany(discovered_output.name.clone())
                        },
                        self.work_directory.clone(),
                        &self.recipe_source.path,
                    ),
                    channels: channels.clone(),
                    channel_priority: Default::default(),
                    solve_strategy: Default::default(),
                    timestamp: chrono::Utc::now(),
                    subpackages: subpackages.clone(),
                    packaging_settings: PackagingSettings::from_args(
                        CondaArchiveType::Conda,
                        CompressionLevel::default(),
                    ),
                    store_recipe: false,
                    force_colors: crate::common::should_force_colors(),
                    sandbox_config: None,
                    debug: Debug::new(false),
                    exclude_newer: None,
                },
                finalized_dependencies: None,
                finalized_cache_dependencies: None,
                finalized_cache_sources: None,
                finalized_sources: None,
                system_tools: SystemTools::new(),
                build_summary: Default::default(),
                extra_meta: None,
            });
        }

        Ok(outputs)
    }

    /// Detect the virtual packages.
    pub fn detect_virtual_packages(
        vpkgs: Option<Vec<GenericVirtualPackage>>,
    ) -> miette::Result<Vec<GenericVirtualPackage>> {
        let vpkgs = match vpkgs {
            Some(vpkgs) => vpkgs,
            None => {
                PlatformWithVirtualPackages::detect(&VirtualPackageOverrides::from_env())
                    .into_diagnostic()?
                    .virtual_packages
            }
        };
        Ok(vpkgs)
    }
}

/// Constructs a `Directories` which tells rattler-build where to place all the
/// different build folders.
///
/// This tries to reduce the number of characters in the path to avoid being too
/// long on Windows.
pub fn output_directory(
    output: OneOrMultipleOutputs,
    work_dir: PathBuf,
    recipe_path: &Path,
) -> Directories {
    let build_dir = match output {
        OneOrMultipleOutputs::Single(_name) => work_dir,
        OneOrMultipleOutputs::OneOfMany(name) => work_dir.join(name),
    };

    let cache_dir = build_dir.join("cache");
    let recipe_dir = recipe_path
        .parent()
        .expect("a recipe *file* must always have a parent directory")
        .to_path_buf();

    let host_prefix = if cfg!(target_os = "windows") {
        build_dir.join("host")
    } else {
        let placeholder_template = "_placehold";
        let mut placeholder = String::new();
        let placeholder_length: usize = 255;

        while placeholder.len() < placeholder_length {
            placeholder.push_str(placeholder_template);
        }

        let placeholder = placeholder
            [0..placeholder_length - build_dir.join("host_env").as_os_str().len()]
            .to_string();

        build_dir.join(format!("host_env{placeholder}"))
    };

    Directories {
        build_dir: build_dir.clone(),
        build_prefix: build_dir.join("bld"),
        cache_dir,
        host_prefix,
        work_dir: build_dir.join("work"),
        recipe_dir,
        recipe_path: recipe_path.to_path_buf(),
        output_dir: build_dir,
    }
}
