use std::{collections::BTreeMap, path::PathBuf, str::FromStr, sync::Arc};

use miette::IntoDiagnostic;
use rattler_build::{
    NormalizedKey,
    metadata::{BuildConfiguration, Debug, Output, PlatformWithVirtualPackages},
    recipe::parser::find_outputs_from_src,
    render::resolved_dependencies::RunExportsDownload,
    selectors::SelectorConfig,
    source_code::Source,
    system_tools::SystemTools,
    tool_configuration,
    types::{Directories, PackageIdentifier, PackagingSettings},
    variant_config::VariantConfig,
};
use rattler_conda_types::compression_level::CompressionLevel;
use rattler_conda_types::{GenericVirtualPackage, NamedChannelOrUrl, package::ArchiveType};
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
    selector_config: SelectorConfig,
    host_virtual_packages: Option<Vec<GenericVirtualPackage>>,
    build_virtual_packages: Option<Vec<GenericVirtualPackage>>,
    channel_base_urls: Option<Vec<Url>>,
    recipe_folder: PathBuf,
    output_dir: PathBuf,
) -> miette::Result<Vec<Output>> {
    let recipe_path = recipe_folder.join("recipe.yaml");

    // First find all outputs from the recipe
    let named_source = Source {
        name: "recipe".to_string(),
        code: Arc::from(
            generated_recipe
                .recipe
                .to_yaml_pretty()
                .into_diagnostic()?
                .as_str(),
        ),
        path: recipe_path.clone(),
    };

    let outputs = find_outputs_from_src(named_source.clone()).into_diagnostic()?;

    let variant_config = VariantConfig::default();

    let outputs_and_variants =
        variant_config.find_variants(&outputs, named_source, &selector_config)?;

    let mut subpackages = BTreeMap::new();
    let mut outputs = Vec::new();

    let global_build_name = outputs_and_variants
        .first()
        .map(|o| o.name.clone())
        .unwrap_or_default();

    for discovered_output in outputs_and_variants {
        let recipe = &discovered_output.recipe;

        if recipe.build().skip() {
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

        let build_name = if recipe.cache.is_some() {
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
                    platform: selector_config.host_platform,
                    virtual_packages: host_virtual_packages.clone().unwrap_or_default(),
                },
                build_platform: PlatformWithVirtualPackages {
                    platform: selector_config.build_platform,
                    virtual_packages: build_virtual_packages.clone().unwrap_or_default(),
                },
                hash: discovered_output.hash.clone(),
                variant: discovered_output.used_vars.clone(),
                directories: Directories::setup(
                    &build_name,
                    &recipe_path,
                    &output_dir,
                    true,
                    &timestamp,
                    recipe.build().merge_build_and_host_envs,
                )
                .into_diagnostic()?,
                channels,
                channel_priority: tool_config.channel_priority,
                timestamp,
                subpackages: subpackages.clone(),
                packaging_settings: PackagingSettings::from_args(
                    ArchiveType::Conda,
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
