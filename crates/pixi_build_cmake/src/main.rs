mod build_script;
mod cmake_lists;
mod config;
mod inputs;
mod metadata;

use build_script::{BuildPlatform, BuildScriptContext};
use config::CMakeBackendConfig;
use metadata::CMakeMetadataProvider;
use miette::IntoDiagnostic;
use pixi_build_backend::{
    compilers::default_compiler_variants,
    generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    tools::BackendIdentifier,
};
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{Item, Script, SerializableMatchSpec, Value};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::{ChannelUrl, Platform};
use std::collections::HashSet;
use std::path::PathBuf;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

#[derive(Default, Clone)]
pub struct CMakeGenerator {}

/// Globs used when ninja-based exact input extraction is unavailable
/// (e.g. the build dir was wiped, ninja exited non-zero, or this is a
/// dry-run). Kept intentionally broad so we don't miss real changes.
fn fallback_input_globs() -> BTreeSet<String> {
    [
        // Source files
        "**/*.{c,cc,cxx,cpp,h,hpp,hxx}",
        // CMake files
        "**/*.{cmake,cmake.in}",
        "**/CMakeLists.txt",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[async_trait::async_trait]
impl GenerateRecipe for CMakeGenerator {
    type Config = CMakeBackendConfig;

    async fn generate_recipe(
        &self,
        model: &pixi_build_types::ProjectModel,
        config: &Self::Config,
        manifest_path: PathBuf,
        _host_platform: Platform,
        _python_params: Option<PythonParams>,
        variants: &HashSet<NormalizedKey>,
        _channels: Vec<ChannelUrl>,
        _cache_dir: Option<PathBuf>,
        _workspace_scratch_directory: Option<PathBuf>,
        _workspace_directory: Option<PathBuf>,
        _checkout_root: Option<PathBuf>,
    ) -> miette::Result<GeneratedRecipe> {
        // Determine the manifest root, because `manifest_path` can be
        // either a direct file path or a directory path.
        let manifest_root = if manifest_path.is_file() {
            manifest_path
                .parent()
                .ok_or_else(|| {
                    miette::Error::msg(format!(
                        "Manifest path {} is a file but has no parent directory.",
                        manifest_path.display()
                    ))
                })?
                .to_path_buf()
        } else {
            manifest_path.clone()
        };

        let mut metadata = CMakeMetadataProvider::new(&manifest_root);

        let mut generated_recipe =
            GeneratedRecipe::from_model(model.clone(), &mut metadata).into_diagnostic()?;

        // we need to add compilers

        let requirements = &mut generated_recipe.recipe.requirements;

        // Take the compilers from the config, or fall back to the languages the
        // project enables in its top level `CMakeLists.txt`.
        let compilers = config
            .compilers
            .clone()
            .unwrap_or_else(|| metadata.compilers());

        // Add configured compilers to build requirements
        pixi_build_backend::compilers::add_compilers_to_requirements(
            &compilers,
            &mut requirements.build,
        );
        pixi_build_backend::compilers::add_stdlib_to_requirements(
            &compilers,
            &mut requirements.build,
            variants,
        );

        // add necessary build tools
        for tool in ["cmake", "ninja"] {
            requirements.build.push(Item::Value(Value::new_concrete(
                SerializableMatchSpec::from(tool),
                None,
            )));
        }

        let build_script = BuildScriptContext {
            build_platform: if Platform::current().is_windows() {
                BuildPlatform::Windows
            } else {
                BuildPlatform::Unix
            },
            source_dir: manifest_root.display().to_string(),
            extra_args: config.extra_args.clone(),
            build_dir: inputs::NINJA_BUILD_DIR,
            toolchain_file_lines: build_script::toolchain_file_lines(&compilers),
        }
        .render();

        *generated_recipe
            .recipe
            .build
            .plan
            .script_mut()
            .expect("generated recipes use script mode") = Script::from_content(build_script)
            .with_env(
                config
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::new_concrete(v.clone(), None)))
                    .collect(),
            )
            .with_secrets(model.secrets.iter().cloned().collect());

        Ok(generated_recipe)
    }

    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<Vec<String>> {
        let workdir = workdir.as_ref();
        let mut globs = match inputs::exact_inputs_from_ninja(workdir) {
            Ok(set) => set,
            Err(err) => {
                tracing::warn!(
                    "falling back to glob-based input tracking for cmake build at {}: {err}",
                    workdir.display()
                );
                fallback_input_globs()
            }
        };
        globs.extend(config.extra_input_globs.iter().cloned());
        Ok(globs.into_iter().collect())
    }

    fn default_variants(
        &self,
        host_platform: Platform,
    ) -> miette::Result<BTreeMap<NormalizedKey, Vec<Variable>>> {
        Ok(default_compiler_variants(host_platform))
    }
}

#[tokio::main]
pub async fn main() {
    if let Err(err) = pixi_build_backend::cli::main(|log| {
        IntermediateBackendInstantiator::<CMakeGenerator>::new(
            BackendIdentifier::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            log,
            Arc::default(),
        )
    })
    .await
    {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use indexmap::IndexMap;
    use pixi_build_backend::{
        protocol::ProtocolInstantiator, utils::test::intermediate_conda_outputs,
    };
    use pixi_build_types::{
        ProjectModel, VariantValue,
        procedures::{conda_outputs::CondaOutputsParams, initialize::InitializeParams},
    };
    use rattler_build_core::console_utils::LoggingOutputHandler;
    use tokio::fs;

    use super::*;

    #[test]
    fn test_input_globs_includes_extra_globs() {
        let config = CMakeBackendConfig {
            extra_input_globs: vec!["custom/*.c".to_string()],
            ..Default::default()
        };

        let generator = CMakeGenerator::default();

        let result = generator.extract_input_globs_from_build(&config, PathBuf::new(), false);

        insta::assert_debug_snapshot!(result);
    }

    #[macro_export]
    macro_rules! project_fixture {
        ($($json:tt)+) => {
            serde_json::from_value::<ProjectModel>(
                serde_json::json!($($json)+)
            ).expect("Failed to create TestProjectModel from JSON fixture.")
        };
    }

    #[tokio::test]
    async fn test_cxx_is_in_build_requirements() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "runDependencies": {
                        "boltons": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig::default(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_env_vars_are_set() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "runDependencies": {
                        "boltons": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let env = IndexMap::from([("foo".to_string(), "bar".to_string())]);

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig {
                    env: env.clone(),
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe.build.plan.script().unwrap(),
        {
            ".content" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_python_probe_is_in_build_script() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {},
            }
        });

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig::default(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // we want to check that
        // -DPython_EXECUTABLE=$PYTHON is set in the build script
        insta::assert_yaml_snapshot!(generated_recipe.recipe.build,

            {
            ".script.content[]" => insta::dynamic_redaction(|value, _path| {
                // content is a ConditionalList<String>, serialized as an array
                if let Some(s) = value.as_str() {
                    assert!(s.lines()
                        .any(|c| c.contains("-DPython_EXECUTABLE")),
                        "expected -DPython_EXECUTABLE in build script, got: {s}"
                    );
                }
                "[content]"
            })
        });
    }

    #[tokio::test]
    async fn test_cxx_is_added_even_if_gcc_is_already_present() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "buildDependencies": {
                        "gxx": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig::default(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // The compiler template is emitted regardless of the manifest
        // dependencies; a user-pinned compiler package coexists with it.
        let has_cxx_compiler = generated_recipe
            .recipe
            .requirements
            .build
            .iter()
            .any(|item| match item {
                Item::Value(value) => value
                    .as_template()
                    .is_some_and(|t| t.to_string() == "${{ compiler('cxx') }}"),
                _ => false,
            });
        assert!(
            has_cxx_compiler,
            "cxx compiler template should be added even when gxx is a build dependency"
        );
    }

    #[tokio::test]
    async fn test_windows_default_compiler() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let factory = IntermediateBackendInstantiator::<CMakeGenerator>::new(
            BackendIdentifier::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
            LoggingOutputHandler::default(),
            Arc::default(),
        )
        .initialize(InitializeParams {
            workspace_directory: None,
            checkout_root: None,
            source_directory: None,
            manifest_path: PathBuf::from("pixi.toml"),
            project_model: Some(project_model),
            configuration: None,
            target_configuration: None,
            cache_directory: None,
            workspace_scratch_directory: None,
        })
        .await
        .unwrap();

        let current_dir = std::env::current_dir().unwrap();
        let outputs = factory
            .0
            .conda_outputs(CondaOutputsParams {
                channels: vec![],
                host_platform: Platform::Win64,
                build_platform: Platform::Win64,
                variant_configuration: None,
                variant_files: None,
                work_directory: current_dir,
            })
            .await
            .unwrap();

        assert_eq!(
            outputs.outputs[0].metadata.variant.get("cxx_compiler"),
            Some(&VariantValue::from("vs2022")),
            "On windows the default cxx_compiler variant should be vs2022"
        );
    }

    #[tokio::test]
    async fn test_default_cuda_compiler() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        for platform in [Platform::Linux64, Platform::Win64] {
            let factory = IntermediateBackendInstantiator::<CMakeGenerator>::new(
                BackendIdentifier::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
                LoggingOutputHandler::default(),
                Arc::default(),
            )
            .initialize(InitializeParams {
                workspace_directory: None,
                checkout_root: None,
                source_directory: None,
                manifest_path: PathBuf::from("pixi.toml"),
                project_model: Some(project_model.clone()),
                configuration: Some(serde_json::json!({ "compilers": ["cuda"] })),
                target_configuration: None,
                cache_directory: None,
                workspace_scratch_directory: None,
            })
            .await
            .unwrap();

            let current_dir = std::env::current_dir().unwrap();
            let outputs = factory
                .0
                .conda_outputs(CondaOutputsParams {
                    channels: vec![],
                    host_platform: platform,
                    build_platform: platform,
                    variant_configuration: None,
                    variant_files: None,
                    work_directory: current_dir,
                })
                .await
                .unwrap();

            assert_eq!(
                outputs.outputs[0].metadata.variant.get("cuda_compiler"),
                Some(&VariantValue::from("cuda-nvcc")),
                "On {platform} the default cuda_compiler variant should be cuda-nvcc",
            );
        }
    }

    #[tokio::test]
    async fn test_intermediate_conda_outputs_snapshot() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                   "buildDependencies": {
                        "boltons": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

        let variant_configuration = BTreeMap::from([(
            "boltons".to_string(),
            Vec::from([VariantValue::from("==1.0.0")]),
        )]);

        let result = intermediate_conda_outputs::<CMakeGenerator>(
            Some(project_model),
            Some(temp_dir.path().to_path_buf()),
            Platform::Linux64,
            Some(variant_configuration),
            None,
        )
        .await;

        assert_eq!(
            result.outputs[0].metadata.variant["boltons"],
            VariantValue::from("==1.0.0")
        );
        if let Some(tp) = result.outputs[0].metadata.variant.get("target_platform") {
            assert_eq!(
                tp,
                &VariantValue::from("linux-64"),
                "Target platform should match the requested platform"
            );
        }
    }

    #[tokio::test]
    async fn test_variant_files_are_applied() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                   "buildDependencies": {
                        "boltons": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

        let variant_file = temp_dir.path().join("variants.yaml");
        fs::write(
            &variant_file,
            r#"boltons:
  - "==2.0.0"
"#,
        )
        .await
        .expect("Failed to write variants file");

        let result = intermediate_conda_outputs::<CMakeGenerator>(
            Some(project_model),
            Some(temp_dir.path().to_path_buf()),
            Platform::Linux64,
            None,
            Some(vec![variant_file]),
        )
        .await;

        assert_eq!(
            result.outputs[0].metadata.variant["boltons"],
            VariantValue::from("==2.0.0")
        );
        if let Some(tp) = result.outputs[0].metadata.variant.get("target_platform") {
            assert_eq!(
                tp,
                &VariantValue::from("linux-64"),
                "Target platform should match the requested platform"
            );
        }
    }

    #[tokio::test]
    async fn test_multiple_compilers_configuration() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig {
                    compilers: Some(vec!["c".to_string(), "cxx".to_string(), "cuda".to_string()]),
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Check that we have exactly the expected compilers
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let compiler_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(value) => value
                    .as_template()
                    .filter(|t| t.to_string().contains("compiler"))
                    .map(|t| t.to_string()),
                _ => None,
            })
            .collect();

        // Should have exactly three compilers
        assert_eq!(
            compiler_templates.len(),
            3,
            "Should have exactly three compilers"
        );

        // Check we have the expected compilers
        assert!(
            compiler_templates.contains(&"${{ compiler('c') }}".to_string()),
            "C compiler should be in build requirements"
        );
        assert!(
            compiler_templates.contains(&"${{ compiler('cxx') }}".to_string()),
            "C++ compiler should be in build requirements"
        );
        assert!(
            compiler_templates.contains(&"${{ compiler('cuda') }}".to_string()),
            "CUDA compiler should be in build requirements"
        );
    }

    /// The version the recipe declares for the package.
    fn package_version(generated_recipe: &GeneratedRecipe) -> String {
        generated_recipe
            .recipe
            .package
            .version
            .as_concrete()
            .expect("the version should be concrete")
            .to_string()
    }

    /// Collects the compilers that the recipe requests through the
    /// `compiler()` template function.
    fn requested_compilers(generated_recipe: &GeneratedRecipe) -> Vec<String> {
        generated_recipe
            .recipe
            .requirements
            .build
            .iter()
            .filter_map(|item| match item {
                Item::Value(value) => value
                    .as_template()
                    .filter(|template| template.to_string().contains("compiler"))
                    .map(|template| template.to_string()),
                _ => None,
            })
            .collect()
    }

    /// Generates a recipe for a manifest root that holds `cmake_lists` as its
    /// top level `CMakeLists.txt`.
    async fn recipe_for_cmake_lists(
        cmake_lists: &str,
        config: CMakeBackendConfig,
        project_model: ProjectModel,
    ) -> GeneratedRecipe {
        let manifest_root = tempfile::tempdir().expect("Failed to create temp dir");
        fs::write(manifest_root.path().join("CMakeLists.txt"), cmake_lists)
            .await
            .expect("Failed to write CMakeLists.txt");

        CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &config,
                manifest_root.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::default(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe")
    }

    #[tokio::test]
    async fn test_compilers_are_taken_from_the_project_languages() {
        let generated_recipe = recipe_for_cmake_lists(
            "project(foobar LANGUAGES C CXX Fortran)",
            CMakeBackendConfig::default(),
            project_fixture!({"name": "foobar", "version": "0.1.0"}),
        )
        .await;

        assert_eq!(
            requested_compilers(&generated_recipe),
            vec![
                "${{ compiler('c') }}".to_string(),
                "${{ compiler('cxx') }}".to_string(),
                "${{ compiler('fortran') }}".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn test_project_without_any_language_needs_no_compiler() {
        let generated_recipe = recipe_for_cmake_lists(
            "project(foobar NONE)",
            CMakeBackendConfig::default(),
            project_fixture!({"name": "foobar", "version": "0.1.0"}),
        )
        .await;

        assert!(requested_compilers(&generated_recipe).is_empty());
    }

    #[tokio::test]
    async fn test_configured_compilers_win_over_the_project_languages() {
        let generated_recipe = recipe_for_cmake_lists(
            "project(foobar LANGUAGES C CXX Fortran)",
            CMakeBackendConfig {
                compilers: Some(vec!["cxx".to_string()]),
                ..Default::default()
            },
            project_fixture!({"name": "foobar", "version": "0.1.0"}),
        )
        .await;

        assert_eq!(
            requested_compilers(&generated_recipe),
            vec!["${{ compiler('cxx') }}".to_string()]
        );
    }

    /// A `CMakeLists.txt` without a `project()` call falls back the same way
    /// as a missing one.
    #[tokio::test]
    async fn test_default_compilers_without_project_call() {
        let generated_recipe = recipe_for_cmake_lists(
            "add_subdirectory(sub)",
            CMakeBackendConfig::default(),
            project_fixture!({"name": "foobar", "version": "0.1.0"}),
        )
        .await;

        assert_eq!(
            requested_compilers(&generated_recipe),
            vec![
                "${{ compiler('c') }}".to_string(),
                "${{ compiler('cxx') }}".to_string(),
            ]
        );
    }

    /// Metadata the manifest leaves out is filled in from the `project()`
    /// call.
    #[tokio::test]
    async fn test_metadata_comes_from_the_project_call() {
        let generated_recipe = recipe_for_cmake_lists(
            r#"project(foobar VERSION 1.2.3 DESCRIPTION "a demo" HOMEPAGE_URL "https://example.com" LANGUAGES CXX)"#,
            CMakeBackendConfig::default(),
            project_fixture!({"name": "foobar"}),
        )
        .await;

        assert_eq!(package_version(&generated_recipe), "1.2.3");

        let about = &generated_recipe.recipe.about;
        assert_eq!(
            about
                .description
                .as_ref()
                .and_then(|value| value.as_concrete())
                .map(String::as_str),
            Some("a demo")
        );
        assert_eq!(
            about
                .homepage
                .as_ref()
                .and_then(|value| value.as_concrete())
                .map(ToString::to_string)
                .as_deref(),
            Some("https://example.com/")
        );
    }

    /// The version of the manifest wins over the one the `project()` call
    /// declares, the same way the other backends treat their manifests.
    #[tokio::test]
    async fn test_the_manifest_version_wins() {
        let generated_recipe = recipe_for_cmake_lists(
            "project(foobar VERSION 1.2.3 LANGUAGES CXX)",
            CMakeBackendConfig::default(),
            project_fixture!({"name": "foobar", "version": "0.1.0"}),
        )
        .await;

        assert_eq!(package_version(&generated_recipe), "0.1.0");
    }

    /// Without a `CMakeLists.txt` to read the languages from, the backend
    /// assumes the languages CMake itself enables by default.
    #[tokio::test]
    async fn test_default_compilers_without_cmake_lists() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig {
                    compilers: None,
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::default(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Check that we have exactly the expected compilers and build tools
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let compiler_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(value) => value
                    .as_template()
                    .filter(|t| t.to_string().contains("compiler"))
                    .map(|t| t.to_string()),
                _ => None,
            })
            .collect();

        assert_eq!(
            compiler_templates,
            vec![
                "${{ compiler('c') }}".to_string(),
                "${{ compiler('cxx') }}".to_string(),
            ],
            "CMake enables C and CXX when it cannot be told otherwise"
        );
    }

    #[tokio::test]
    async fn test_stdlib_is_added() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig {
                    compilers: None,
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::from_iter([NormalizedKey("c_stdlib".into())]),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Check that we have exactly the expected compilers and build tools
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let stdlib_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(value) => value
                    .as_template()
                    .filter(|t| t.to_string().contains("stdlib"))
                    .map(|t| t.to_string()),
                _ => None,
            })
            .collect();

        // Should have exactly one compiler: cxx
        assert_eq!(stdlib_templates.len(), 1, "Should have exactly one stdlib");
        assert_eq!(
            stdlib_templates[0], "${{ stdlib('c') }}",
            "Default stdlib should be c"
        );
    }
}
