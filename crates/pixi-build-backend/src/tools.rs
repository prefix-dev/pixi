use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use indexmap::IndexSet;
use itertools::Itertools;
use miette::IntoDiagnostic;
use rattler_build::{
    hash::HashInfo,
    metadata::{BuildConfiguration, Debug, Output, PlatformWithVirtualPackages},
    recipe::{
        Jinja, ParsingError, Recipe,
        parser::{BuildString, GlobVec, find_outputs_from_src},
    },
    selectors::SelectorConfig,
    system_tools::SystemTools,
    types::{Directories, PackageIdentifier, PackagingSettings},
    variant_config::{DiscoveredOutput, ParseErrors, VariantConfig, VariantConfigError},
};
use rattler_conda_types::compression_level::CompressionLevel;
use rattler_conda_types::{GenericVirtualPackage, Platform, package::ArchiveType};
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
    /// The selector configuration.
    pub selector_config: SelectorConfig,
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
        selector_config: &SelectorConfig,
        additional_variant_files: impl Iterator<Item = &'a Path>,
    ) -> Result<Self, VariantConfigError<Arc<str>>> {
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
            variant_config: VariantConfig::from_files(&variant_files, selector_config)?,
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
    pub fn new(source: Source, selector_config: SelectorConfig, work_directory: PathBuf) -> Self {
        Self {
            recipe_source: source,
            selector_config,
            work_directory,
        }
    }

    /// Discover the outputs from the recipe.
    pub fn discover_outputs(
        &self,
        variant_files: &[PathBuf],
        variant_config_input: &Option<BTreeMap<String, Vec<String>>>,
    ) -> miette::Result<IndexSet<DiscoveredOutput>> {
        // First find all outputs from the recipe
        let outputs = find_outputs_from_src(self.recipe_source.clone())?;

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
            VariantConfig::from_files(&variant_config_paths, &self.selector_config)?;

        if let Some(variant_config_input) = variant_config_input {
            for (k, v) in variant_config_input.iter() {
                let variables = v.iter().map(|v| v.clone().into()).collect();
                variant_config.variants.insert(k.as_str().into(), variables);
            }
        }

        Ok(variant_config.find_variants(
            &outputs,
            self.recipe_source.clone(),
            &self.selector_config,
        )?)
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

        let channels = channels.into_iter().map(Into::into).collect_vec();
        for discovered_output in discovered_outputs {
            let hash = HashInfo::from_variant(
                &discovered_output.used_vars,
                &discovered_output.noarch_type,
            );

            let selector_config = SelectorConfig {
                variant: discovered_output.used_vars.clone(),
                hash: Some(hash.clone()),
                target_platform: self.selector_config.target_platform,
                host_platform: self.selector_config.host_platform,
                build_platform: self.selector_config.build_platform,
                experimental: true,
                allow_undefined: false,
                recipe_path: Some(self.recipe_source.path.clone()),
            };

            let mut recipe = Recipe::from_node(&discovered_output.node, selector_config.clone())
                .map_err(|err| {
                    let errs: ParseErrors<_> = err
                        .into_iter()
                        .map(|err| ParsingError::from_partial(self.recipe_source.clone(), err))
                        .collect::<Vec<_>>()
                        .into();
                    errs
                })?;

            recipe.build.string = BuildString::Resolved(BuildString::compute(
                &discovered_output.hash,
                recipe.build.number,
            ));

            for source in &mut recipe.source {
                if let rattler_build::recipe::parser::Source::Path(path_source) = source {
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

            if recipe.build().skip() {
                eprintln!(
                    "Skipping build for variant: {:#?}",
                    discovered_output.used_vars
                );
                continue;
            }

            let jinja = Jinja::new(selector_config);

            subpackages.insert(
                recipe.package().name().clone(),
                PackageIdentifier {
                    name: recipe.package().name().clone(),
                    version: recipe.package().version().version().clone().into(),
                    build_string: recipe
                        .build()
                        .string()
                        .resolve(&hash, recipe.build().number(), &jinja)
                        .into_owned(),
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
                    hash,
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
                        ArchiveType::Conda,
                        CompressionLevel::default(),
                    ),
                    store_recipe: false,
                    force_colors: true,
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
