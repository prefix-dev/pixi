use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use pixi_build_types::{
    VariantValue,
    procedures::{
        conda_outputs::{CondaOutputsParams, CondaOutputsResult},
        initialize::InitializeParams,
    },
};
use rattler_build::console_utils::LoggingOutputHandler;
use rattler_conda_types::Platform;
use serde_json::Value;

use crate::{
    generated_recipe::GenerateRecipe, intermediate_backend::IntermediateBackendInstantiator,
    protocol::ProtocolInstantiator,
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
    intermediate_conda_outputs_cross::<T>(
        project_model,
        source_dir,
        host_platform,
        host_platform,
        variant_configuration,
        variant_files,
    )
    .await
}

/// Like [`intermediate_conda_outputs`] but allows specifying a different build
/// platform (for cross-compilation testing).
pub async fn intermediate_conda_outputs_cross<T>(
    project_model: Option<pixi_build_types::ProjectModel>,
    source_dir: Option<PathBuf>,
    host_platform: Platform,
    build_platform: Platform,
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
        LoggingOutputHandler::default(),
        Arc::new(T::default()),
    )
    .initialize(InitializeParams {
        workspace_directory: None,
        source_directory: source_dir,
        manifest_path,
        project_model,
        configuration: None,
        target_configuration: None,
        cache_directory: None,
    })
    .await
    .unwrap();

    let current_dir = std::env::current_dir().unwrap();
    protocol
        .conda_outputs(CondaOutputsParams {
            channels: vec![],
            host_platform,
            build_platform,
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
