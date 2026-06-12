use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use pixi_build_types::{
    VariantValue,
    procedures::{
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
        initialize::InitializeParams,
    },
};
use rattler_build_core::console_utils::LoggingOutputHandler;
use rattler_conda_types::Platform;
use serde_json::Value;

use crate::{
    generated_recipe::GenerateRecipe, intermediate_backend::IntermediateBackendInstantiator,
    protocol::ProtocolInstantiator, tools::BackendIdentifier,
};

/// A utility function to remove empty values from a JSON object.
pub(crate) fn remove_empty_values(value: &mut Value) {
    fn keep_value(value: &Value) -> bool {
        match value {
            Value::Object(map) => !map.is_empty(),
            Value::Array(arr) => !arr.is_empty(),
            Value::Null => false,
            _ => true,
        }
    }

    match value {
        Value::Object(map) => {
            map.retain(|_, v| {
                remove_empty_values(v);
                keep_value(v)
            });
        }
        Value::Array(arr) => {
            arr.retain_mut(|v| {
                remove_empty_values(v);
                keep_value(v)
            });
        }
        _ => {}
    }
}

/// Prepares and calls the `conda/outputs` procedure of the `IntermediateBackend` for the
/// given recipe general and with the given project model.
pub async fn intermediate_conda_outputs<T>(
    project_model: Option<pixi_build_types::ProjectModel>,
    source_dir: Option<PathBuf>,
    host_platform: Platform,
    variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,
    variant_files: Option<Vec<PathBuf>>,
) -> CondaOutputsResult
where
    T: GenerateRecipe + Default + Clone + Send + Sync + 'static,
    <T as GenerateRecipe>::Config: Send + Sync + 'static,
{
    let manifest_path = match &source_dir {
        Some(dir) => dir.join("pixi.toml"),
        None => PathBuf::from("pixi.toml"),
    };

    let (protocol, _result) = IntermediateBackendInstantiator::<T>::new(
        BackendIdentifier::new("test-backend", env!("CARGO_PKG_VERSION")),
        LoggingOutputHandler::default(),
        Arc::new(T::default()),
    )
    .initialize(InitializeParams {
        workspace_directory: None,
        checkout_root: None,
        source_directory: source_dir,
        manifest_path,
        project_model,
        configuration: None,
        target_configuration: None,
        cache_directory: None,
        workspace_scratch_directory: None,
    })
    .await
    .unwrap();

    let current_dir = std::env::current_dir().unwrap();
    protocol
        .conda_outputs(CondaOutputsParams {
            channels: vec![],
            host_platform,
            build_platform: host_platform,
            variant_configuration,
            variant_files,
            work_directory: current_dir,
        })
        .await
        .unwrap()
}

/// A function to convert a `CondaOutputsResult` into a pretty-printed JSON
/// string.
pub fn conda_outputs_snapshot(result: CondaOutputsResult) -> String {
    let mut value = serde_json::to_value(result).unwrap();
    remove_empty_values(&mut value);
    serde_json::to_string_pretty(&value).unwrap()
}

/// Renders a generated recipe the same way the build procedures do and
/// returns the recipe of the first output that is not skipped. Intended for
/// backend tests that exercise
/// [`crate::generated_recipe::GenerateRecipe::finalize_build_script`].
pub fn render_generated_recipe(
    generated_recipe: &crate::generated_recipe::GeneratedRecipe,
    host_platform: Platform,
) -> rattler_build_recipe::stage1::Recipe {
    let recipe_code = serde_yaml::to_string(&generated_recipe.recipe).unwrap();
    let source = rattler_build_recipe::source_code::Source::from_string(
        "recipe.yaml".to_string(),
        recipe_code,
    );

    let repodata_revision = if crate::v3::generated_recipe_uses_v3(&generated_recipe.recipe) {
        rattler_conda_types::RepodataRevision::V3
    } else {
        rattler_conda_types::RepodataRevision::Legacy
    };

    let stage0_recipe = rattler_build_recipe::parse_recipe_with_config(
        &source,
        rattler_build_recipe::stage0::ParseConfig { repodata_revision },
    )
    .unwrap();

    let variant_config = rattler_build_variant_config::VariantConfig {
        variants: BTreeMap::new(),
        zip_keys: None,
    };
    let render_config = rattler_build_recipe::variant_render::RenderConfig::new()
        .with_target_platform(host_platform)
        .with_build_platform(host_platform)
        .with_host_platform(host_platform)
        .with_repodata_revision(repodata_revision);

    rattler_build_recipe::render_recipe(&source, &stage0_recipe, &variant_config, render_config)
        .unwrap()
        .into_iter()
        .map(|rendered| rendered.recipe)
        .find(|recipe| !recipe.build().skip)
        .expect("the recipe should render at least one output that is not skipped")
}
