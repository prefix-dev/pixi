mod build_script;
mod config;
mod inputs;

use build_script::{BuildPlatform, BuildScriptContext};
use config::{CMakeBackendConfig, CompilerCache, CompilerCacheConfig};
use miette::IntoDiagnostic;
use pixi_build_backend::{
    cache::{ensure_compiler_cache_on_path, sccache_envs, sccache_tools},
    compilers::default_compiler_variants,
    generated_recipe::{DefaultMetadataProvider, GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    tools::BackendIdentifier,
    traits::ProjectModel,
};
use pixi_build_types::SourcePackageName;
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{Item, Script, SerializableMatchSpec, Value};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::PackageName;
use rattler_conda_types::{ChannelUrl, Platform};
use std::collections::{HashMap, HashSet};
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
        host_platform: Platform,
        _python_params: Option<PythonParams>,
        variants: &HashSet<NormalizedKey>,
        _channels: Vec<ChannelUrl>,
        _cache_dir: Option<PathBuf>,
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

        let mut generated_recipe =
            GeneratedRecipe::from_model(model.clone(), &mut DefaultMetadataProvider)
                .into_diagnostic()?;

        // we need to add compilers

        let requirements = &mut generated_recipe.recipe.requirements;

        // Get the platform-specific dependencies from the project model.
        // This properly handles target selectors like [target.linux-64] by using
        // the ProjectModel trait's platform-aware API instead of trying to evaluate
        // rattler-build selectors with simple string comparison.
        let model_dependencies = model.dependencies(Some(host_platform));

        // Get the list of compilers from config, defaulting to ["cxx"] if not specified
        let compilers = config
            .compilers
            .clone()
            .unwrap_or_else(|| vec!["cxx".to_string()]);

        // Add configured compilers to build requirements
        pixi_build_backend::compilers::add_compilers_to_requirements(
            &compilers,
            &mut requirements.build,
            &model_dependencies,
            &host_platform,
        );
        pixi_build_backend::compilers::add_stdlib_to_requirements(
            &compilers,
            &mut requirements.build,
            variants,
        );

        // add necessary build tools
        for tool in ["cmake", "ninja"] {
            let tool_name = SourcePackageName::from(PackageName::new_unchecked(tool));
            if !model_dependencies.build.contains_key(&tool_name) {
                requirements.build.push(Item::Value(Value::new_concrete(
                    SerializableMatchSpec::from(tool),
                    None,
                )));
            }
        }

        // Check if the host platform has a host python dependency
        // This is used to determine if we need to the cmake argument for the python
        // executable
        let has_host_python = model_dependencies
            .host
            .contains_key(&SourcePackageName::from(PackageName::new_unchecked(
                "python",
            )));

        // Enable sccache when the resolved configuration requests it. Where the
        // setting came from decides how the tool is provided: a package-local
        // `compiler-cache` is added to the build requirements (and therefore
        // the lockfile) so the build is reproducible everywhere, while a
        // globally-injected default is a per-machine preference used as a
        // launcher only — adding it to the requirements would make the lockfile
        // flip-flop depending on who runs the resolve, so the tool must already
        // be on `PATH` instead.
        let has_sccache = matches!(
            config.compiler_cache.as_ref().map(CompilerCacheConfig::cache),
            Some(CompilerCache::Sccache)
        );
        let mut sccache_secrets: BTreeSet<String> = BTreeSet::new();

        if let Some(compiler_cache) = &config.compiler_cache {
            // Mark any `SCCACHE_*` variables present in the system environment
            // (but not explicitly set in the backend config `env`) as secrets so
            // they are not leaked into the build recipe.
            let system_env_vars: HashMap<String, String> = config
                .system_env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if let Some(system_sccache_keys) = sccache_envs(&system_env_vars) {
                sccache_secrets = config
                    .system_env
                    .keys()
                    .filter(|key| {
                        system_sccache_keys.contains(&key.as_str())
                            && !config.env.contains_key(*key)
                    })
                    .cloned()
                    .collect();
            }

            if compiler_cache.lock_as_dependency() {
                // Add sccache to the build requirements if not already present.
                let sccache_dep: Vec<Item<SerializableMatchSpec>> = sccache_tools()
                    .iter()
                    .map(|tool| {
                        Item::Value(Value::new_concrete(
                            SerializableMatchSpec::from(tool.as_str()),
                            None,
                        ))
                    })
                    .collect();
                let existing_reqs: Vec<_> = requirements.build.clone().into_iter().collect();
                requirements.build.extend(
                    sccache_dep
                        .into_iter()
                        .filter(|dep| !existing_reqs.contains(dep)),
                );
            } else {
                // Globally configured: leave the locked build requirements
                // untouched and require the tool to be installed on the machine.
                ensure_compiler_cache_on_path(&sccache_tools())?;
            }
        }

        let build_script = BuildScriptContext {
            build_platform: if Platform::current().is_windows() {
                BuildPlatform::Windows
            } else {
                BuildPlatform::Unix
            },
            source_dir: manifest_root.display().to_string(),
            extra_args: config.extra_args.clone(),
            has_host_python,
            has_sccache,
        }
        .render();

        sccache_secrets.extend(model.secrets.iter().cloned());

        generated_recipe.recipe.build.script = Script::from_content(build_script)
            .with_env(
                config
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::new_concrete(v.clone(), None)))
                    .collect(),
            )
            .with_secrets(sccache_secrets.into_iter().collect());

        Ok(generated_recipe)
    }

    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<BTreeSet<String>> {
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
        Ok(globs)
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
            )
            .await
            .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe.build.script,
        {
            ".content" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_has_python_is_set_in_build_script() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {
                        "python": {
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
    async fn test_cxx_is_not_added_if_gcc_is_already_present() {
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
            )
            .await
            .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script" => "[ ... script ... ]",
        });
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
            source_directory: None,
            manifest_path: PathBuf::from("pixi.toml"),
            project_model: Some(project_model),
            configuration: None,
            target_configuration: None,
            cache_directory: None,
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
                source_directory: None,
                manifest_path: PathBuf::from("pixi.toml"),
                project_model: Some(project_model.clone()),
                configuration: Some(serde_json::json!({ "compilers": ["cuda"] })),
                target_configuration: None,
                cache_directory: None,
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

    #[tokio::test]
    async fn test_default_compiler_when_not_specified() {
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

        // Should have exactly one compiler: cxx
        assert_eq!(
            compiler_templates.len(),
            1,
            "Should have exactly one compiler when not specified"
        );
        assert_eq!(
            compiler_templates[0], "${{ compiler('cxx') }}",
            "Default compiler should be cxx"
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

    #[tokio::test]
    async fn test_sccache_is_enabled() {
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

        // SCCACHE_* env vars in system_env should be marked as secrets when
        // compiler_cache is set to sccache.
        let env = IndexMap::from([("SCCACHE_BUCKET".to_string(), "my-bucket".to_string())]);
        let system_env = IndexMap::from([
            ("SCCACHE_SYSTEM".to_string(), "SOME_VALUE".to_string()),
            ("SCCACHE_BUCKET".to_string(), "system-bucket".to_string()),
        ]);

        let generated_recipe = CMakeGenerator::default()
            .generate_recipe(
                &project_model,
                &CMakeBackendConfig {
                    env,
                    system_env,
                    compiler_cache: Some(CompilerCacheConfig::Package(CompilerCache::Sccache)),
                    ..CMakeBackendConfig::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Verify that sccache is added to the build requirements and the
        // system SCCACHE_* variables are recorded as secrets when
        // compiler_cache = "sccache" is set.
        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script.content" => "[ ... script ... ]",
        });
    }
}
