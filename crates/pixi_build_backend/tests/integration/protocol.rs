use std::sync::Arc;

use crate::common::model::{convert_test_model_to_project_model_v1, load_project_model_from_json};
use imp::TestGenerateRecipe;
use pixi_build_backend::{intermediate_backend::IntermediateBackend, protocol::Protocol};
use pixi_build_types::procedures::conda_build_v1::{CondaBuildV1Output, CondaBuildV1Params};
use rattler_build::console_utils::LoggingOutputHandler;
use rattler_conda_types::{ChannelUrl, Platform};
use serde_json::json;
use tempfile::TempDir;
use url::Url;

#[cfg(test)]
mod imp {
    use miette::IntoDiagnostic;
    use pixi_build_backend::generated_recipe::{
        BackendConfig, DefaultMetadataProvider, GenerateRecipe, GeneratedRecipe, PythonParams,
    };
    use rattler_conda_types::ChannelUrl;
    use serde::{Deserialize, Serialize};
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
    };

    #[derive(Debug, Default, Serialize, Deserialize, Clone)]
    #[serde(rename_all = "kebab-case")]
    pub struct TestBackendConfig {
        /// If set, internal state will be logged as files in that directory
        #[serde(alias = "debug_dir")]
        pub debug_dir: Option<PathBuf>,
    }

    #[cfg(test)]
    #[derive(Clone, Default)]
    pub(crate) struct TestGenerateRecipe {}

    impl BackendConfig for TestBackendConfig {
        fn debug_dir(&self) -> Option<&Path> {
            self.debug_dir.as_deref()
        }

        fn merge_with_target_config(&self, target_config: &Self) -> miette::Result<Self> {
            if target_config.debug_dir.is_some() {
                miette::bail!("`debug_dir` cannot have a target specific value");
            }

            Ok(Self {
                debug_dir: self.debug_dir.clone(),
            })
        }
    }

    #[async_trait::async_trait]
    impl GenerateRecipe for TestGenerateRecipe {
        type Config = TestBackendConfig;

        async fn generate_recipe(
            &self,
            model: &pixi_build_types::ProjectModel,
            _config: &Self::Config,
            _manifest_path: PathBuf,
            _host_platform: rattler_conda_types::Platform,
            _python_params: Option<PythonParams>,
            _variants: &HashSet<pixi_build_backend::variants::NormalizedKey>,
            _channels: Vec<ChannelUrl>,
            _cache_dir: Option<PathBuf>,
        ) -> miette::Result<GeneratedRecipe> {
            GeneratedRecipe::from_model(model.clone(), &mut DefaultMetadataProvider)
                .into_diagnostic()
        }
    }
}

#[tokio::test]
#[ignore] // TODO: this test since it sometimes hangs in CI
async fn test_conda_build_v1() {
    let tmp_dir = TempDir::new().unwrap();
    let tmp_dir_path = tmp_dir.path().to_path_buf();

    let pixi_manifest = tmp_dir_path.join("pixi.toml");
    let build_dir = tmp_dir_path.join("build");

    // Load a model from JSON
    let original_model = load_project_model_from_json("minimal_project_model_for_build.json");

    // Serialize it back to JSON
    let project_model_v1 = convert_test_model_to_project_model_v1(original_model);

    // save the pixi.toml file to a temporary location
    fs_err::write(&pixi_manifest, toml::to_string(&project_model_v1).unwrap()).unwrap();

    let channel_url = Url::parse("https://prefix.dev/conda-forge").unwrap();

    let channel_url = ChannelUrl::from(channel_url);

    let build_params = CondaBuildV1Params {
        channels: vec![channel_url],

        build_prefix: None,
        host_prefix: None,
        run_constraints: None,
        run_dependencies: None,
        run_exports: None,
        output: CondaBuildV1Output {
            name: "minimal-package".parse().unwrap(),
            version: None,
            build: None,
            subdir: Platform::current(),
            variant: Default::default(),
        },
        work_directory: build_dir.clone(),
        output_directory: None,
        editable: None,
    };

    let some_config = json!({
        "debug-dir": "some_debug_dir",
    });

    let target_config = Default::default();

    let intermediate_backend: IntermediateBackend<TestGenerateRecipe> = IntermediateBackend::new(
        pixi_manifest.clone(),
        Some(tmp_dir_path.clone()),
        project_model_v1,
        Arc::default(),
        some_config,
        target_config,
        LoggingOutputHandler::default(),
        None,
    )
    .unwrap();

    let conda_build_result = intermediate_backend
        .conda_build_v1(build_params)
        .await
        .unwrap();

    insta::assert_yaml_snapshot!(conda_build_result, {
        ".output_file" => "[redacted]",
        ".build" => "[redacted]",
        ".subdir" => "[redacted]",
    });

    assert!(build_dir.join("debug").join("recipe.yaml").exists());
}
