use std::{collections::BTreeMap, path::PathBuf, str::FromStr, sync::Arc};

use miette::IntoDiagnostic;
use rattler_build::{
    DiscoveredOutput,
    metadata::{BuildConfiguration, Debug, Output, PlatformWithVirtualPackages},
    render::resolved_dependencies::RunExportsDownload,
    system_tools::SystemTools,
    tool_configuration,
    types::{Directories, PackageIdentifier, PackagingSettings},
};
use rattler_build_recipe::variant_render::RenderConfig;
use rattler_build_types::NormalizedKey;
use rattler_build_variant_config::VariantConfig;
use rattler_conda_types::compression_level::CompressionLevel;
use rattler_conda_types::{
    GenericVirtualPackage, NamedChannelOrUrl, NoArchType, Platform, package::CondaArchiveType,
};
use url::Url;

use crate::{generated_recipe::GeneratedRecipe, utils::TemporaryRenderedRecipe};

/// A very similar function to `get_build_output` from rattler-build.
/// The difference is that in rattler-build, the function should load the recipe from a file.
/// Here, we already have the recipe in memory as a `GeneratedRecipe` and try to ingest it
/// to rattler-build as a string.
/// As a future work, `get_build_output` from rattler-build should be refactored to
/// use the `IntermediateRecipe` directly.
#[allow(clippy::too_many_arguments)]
pub async fn get_build_output(
    generated_recipe: &GeneratedRecipe,
    tool_config: Arc<tool_configuration::Configuration>,
    target_platform: Platform,
    host_platform: Platform,
    build_platform: Platform,
    host_virtual_packages: Option<Vec<GenericVirtualPackage>>,
    build_virtual_packages: Option<Vec<GenericVirtualPackage>>,
    channel_base_urls: Option<Vec<Url>>,
    recipe_folder: PathBuf,
    output_dir: PathBuf,
) -> miette::Result<Vec<Output>> {
    let recipe_path = recipe_folder.join("recipe.yaml");
    let recipe_code = generated_recipe.recipe.to_yaml_pretty().into_diagnostic()?;

    // Create source for error reporting
    let source = rattler_build_recipe::source_code::Source::from_string(
        "recipe".to_string(),
        recipe_code.clone(),
    );

    // Parse the recipe into stage0
    let stage0_recipe = rattler_build_recipe::parse_recipe(&source)?;

    let variant_config = VariantConfig::default();

    // Build render config
    let render_config = RenderConfig::new()
        .with_target_platform(target_platform)
        .with_build_platform(build_platform)
        .with_host_platform(host_platform)
        .with_recipe_path(&recipe_path);

    // Render recipe with variant config
    let rendered_variants = rattler_build_recipe::render_recipe(
        &source,
        &stage0_recipe,
        &variant_config,
        render_config,
    )?;

    // Convert to DiscoveredOutputs
    let outputs_and_variants: Vec<DiscoveredOutput> = rendered_variants
        .into_iter()
        .map(|rendered| {
            let recipe = rendered.recipe;
            let variant = rendered.variant;
            let effective_target_platform = if recipe.build().noarch.is_none() {
                target_platform
            } else {
                Platform::NoArch
            };
            let build_string = recipe
                .build()
                .string
                .as_resolved()
                .expect("build string should be resolved")
                .to_string();
            DiscoveredOutput {
                name: recipe.package().name().as_normalized().to_string(),
                version: recipe.package().version().to_string(),
                build_string,
                noarch_type: recipe.build().noarch.unwrap_or(NoArchType::none()),
                target_platform: effective_target_platform,
                used_vars: variant,
                recipe,
                hash: rendered.hash_info.expect("hash should be set"),
            }
        })
        .collect();

    let mut subpackages = BTreeMap::new();
    let mut outputs = Vec::new();

    let global_build_name = outputs_and_variants
        .first()
        .map(|o| o.name.clone())
        .unwrap_or_default();

    for discovered_output in outputs_and_variants {
        let recipe = &discovered_output.recipe;

        if recipe.build().skip {
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

        let build_name = if recipe.inherits_from.is_some() {
            global_build_name.clone()
        } else {
            recipe.package().name().as_normalized().to_string()
        };

        let variant_channels = if let Some(channel_sources) = discovered_output
            .used_vars
            .get(&NormalizedKey("channel_sources".to_string()))
        {
            Some(
                channel_sources
                    .to_string()
                    .split(',')
                    .map(str::trim)
                    .map(|s| NamedChannelOrUrl::from_str(s).into_diagnostic())
                    .collect::<miette::Result<Vec<_>>>()?,
            )
        } else {
            None
        };

        let channels = variant_channels.unwrap_or_else(|| {
            channel_base_urls
                .as_ref()
                .map(|arg| arg.iter().cloned().map(NamedChannelOrUrl::Url).collect())
                .unwrap_or_else(|| vec![NamedChannelOrUrl::Name("conda-forge".to_string())])
        });

        let channels = channels
            .into_iter()
            .map(|c| c.into_base_url(&tool_config.channel_config))
            .collect::<Result<Vec<_>, _>>()
            .into_diagnostic()?;

        let timestamp = chrono::Utc::now();

        let output = Output {
            recipe: recipe.clone(),
            build_configuration: BuildConfiguration {
                target_platform: discovered_output.target_platform,
                host_platform: PlatformWithVirtualPackages {
                    platform: host_platform,
                    virtual_packages: host_virtual_packages.clone().unwrap_or_default(),
                },
                build_platform: PlatformWithVirtualPackages {
                    platform: build_platform,
                    virtual_packages: build_virtual_packages.clone().unwrap_or_default(),
                },
                hash: discovered_output.hash.clone(),
                variant: discovered_output.used_vars.clone(),
                directories: Directories::builder(
                    &build_name,
                    &recipe_path,
                    &output_dir,
                    &timestamp,
                )
                .no_build_id(true)
                .merge_build_and_host(recipe.build().merge_build_and_host_envs)
                .build()
                .into_diagnostic()?,
                channels,
                channel_priority: tool_config.channel_priority,
                timestamp,
                subpackages: subpackages.clone(),
                packaging_settings: PackagingSettings::from_args(
                    CondaArchiveType::Conda,
                    CompressionLevel::default(),
                ),
                store_recipe: false,
                force_colors: false,
                sandbox_config: None,
                debug: Debug::default(),
                solve_strategy: Default::default(),
                exclude_newer: None,
            },
            finalized_dependencies: None,
            finalized_sources: None,
            finalized_cache_dependencies: None,
            finalized_cache_sources: None,
            system_tools: SystemTools::default(),
            build_summary: Arc::default(),
            extra_meta: None,
        };

        let temp_recipe = TemporaryRenderedRecipe::from_output(&output)?;
        let tool_config = tool_config.clone();
        let output = temp_recipe
            .within_context_async(move || async move {
                output
                    .resolve_dependencies(&tool_config, RunExportsDownload::DownloadMissing)
                    .await
                    .into_diagnostic()
            })
            .await?;

        outputs.push(output);
    }

    Ok(outputs)
}
