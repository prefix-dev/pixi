mod build_script;
mod config;

use build_script::{BuildPlatform, BuildScriptContext};
use config::CMakeBackendConfig;
use miette::IntoDiagnostic;
use pixi_build_backend::{
    generated_recipe::{DefaultMetadataProvider, GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    traits::ProjectModel,
};
use pixi_build_types::SourcePackageName;
use rattler_build_jinja::Variable;
use rattler_build_types::NormalizedKey;
use rattler_conda_types::{ChannelUrl, Platform};
use recipe_stage0::recipe::Script;
use std::collections::HashSet;
use std::path::PathBuf;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

#[derive(Default, Clone)]
pub struct CMakeGenerator {}

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
            let tool_name = SourcePackageName::from(tool);
            if !model_dependencies.build.contains_key(&tool_name) {
                requirements.build.push(tool.parse().into_diagnostic()?);
            }
        }

        // Check if the host platform has a host python dependency
        // This is used to determine if we need to the cmake argument for the python
        // executable
        let has_host_python = model_dependencies
            .host
            .contains_key(&SourcePackageName::from("python"));

        let build_script = BuildScriptContext {
            build_platform: if Platform::current().is_windows() {
                BuildPlatform::Windows
            } else {
                BuildPlatform::Unix
            },
            source_dir: manifest_root.display().to_string(),
            extra_args: config.extra_args.clone(),
            has_host_python,
        }
        .render();

        generated_recipe.recipe.build.script = Script {
            content: build_script,
            env: config.env.clone(),
            ..Default::default()
        };

        Ok(generated_recipe)
    }

    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<BTreeSet<String>> {
        Ok([
            // Source files
            "**/*.{c,cc,cxx,cpp,h,hpp,hxx}",
            // CMake files
            "**/*.{cmake,cmake.in}",
            "**/CMakeFiles.txt",
        ]
        .iter()
        .map(|s: &&str| s.to_string())
        .chain(config.extra_input_globs.clone())
        .collect())
    }

    fn default_variants(
        &self,
        host_platform: Platform,
    ) -> miette::Result<BTreeMap<NormalizedKey, Vec<Variable>>> {
        let mut variants = BTreeMap::new();

        if host_platform.is_windows() {
            // Default to the Visual Studio 2022 compiler on Windows
            // Not 2019 due to Conda-forge switching and the mainstream support dropping in 2024.
            // rattler-build will default to vs2017 which for most github runners is too
            // old.
            variants.insert(NormalizedKey::from("c_compiler"), vec!["vs2022".into()]);
            variants.insert(NormalizedKey::from("cxx_compiler"), vec!["vs2022".into()]);
        }

        Ok(variants)
    }
}

#[tokio::main]
pub async fn main() {
    if let Err(err) = pixi_build_backend::cli::main(|log| {
        IntermediateBackendInstantiator::<CMakeGenerator>::new(log, Arc::default())
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
    use rattler_build::console_utils::LoggingOutputHandler;
    use recipe_stage0::recipe::{Item, Value};
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
            ".script.content" => insta::dynamic_redaction(|value, _path| {
                dbg!(&value);
                // assert that the value looks like a uuid here
                assert!(value.as_str().unwrap().lines()
                    .any(|c| c.contains("-DPython_EXECUTABLE"))
                );
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
                Item::Value(Value::Template(s)) if s.contains("compiler") => Some(s.clone()),
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
                Item::Value(Value::Template(s)) if s.contains("compiler") => Some(s.clone()),
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

    /// Regression test for https://github.com/prefix-dev/pixi/issues/5562
    ///
    /// When cross-compiling (e.g. building for osx-arm64 on osx-64), target-specific
    /// build dependencies like `[package.target.osx-64.build-dependencies]` should
    /// be resolved against build_platform, not host_platform.
    #[tokio::test]
    async fn test_cross_compile_target_specific_build_deps() {
        use pixi_build_backend::utils::test::intermediate_conda_outputs_cross;

        // Create a project with:
        // - A default build dep (should always be included)
        // - An osx-64 target-specific build dep (sigtool - should be included when
        //   build_platform is osx-64)
        // - An osx-arm64 target-specific host dep (should be included when
        //   host_platform is osx-arm64)
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "buildDependencies": {
                        "make": {
                            "binary": {}
                        }
                    },
                    "hostDependencies": {
                        "zlib": {
                            "binary": {}
                        }
                    }
                },
                "targets": {
                    "osx-64": {
                        "buildDependencies": {
                            "sigtool": {
                                "binary": {
                                    "version": ">=0.1.3,<0.2"
                                }
                            }
                        }
                    },
                    "osx-arm64": {
                        "hostDependencies": {
                            "libfoo": {
                                "binary": {}
                            }
                        }
                    }
                }
            }
        });

        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

        // Cross-compile: host=osx-arm64 (target), build=osx-64 (current machine)
        let result = intermediate_conda_outputs_cross::<CMakeGenerator>(
            Some(project_model),
            Some(temp_dir.path().to_path_buf()),
            Platform::OsxArm64, // host_platform (target)
            Platform::Osx64,    // build_platform (current machine)
            None,
            None,
        )
        .await;

        assert!(
            !result.outputs.is_empty(),
            "Should have at least one output"
        );
        let output = &result.outputs[0];

        // Check build dependencies: should contain sigtool (from osx-64 build dep,
        // since build_platform is osx-64) and make (from default target)
        let build_dep_names: Vec<_> = output
            .build_dependencies
            .as_ref()
            .unwrap()
            .depends
            .iter()
            .map(|dep| dep.name.as_str())
            .collect();

        assert!(
            build_dep_names.contains(&"sigtool"),
            "sigtool should be in build dependencies when build_platform is osx-64, \
             got: {build_dep_names:?}"
        );

        // Check host dependencies: should contain libfoo (from osx-arm64 host dep,
        // since host_platform is osx-arm64) and zlib (from default target)
        let host_dep_names: Vec<_> = output
            .host_dependencies
            .as_ref()
            .unwrap()
            .depends
            .iter()
            .map(|dep| dep.name.as_str())
            .collect();

        assert!(
            host_dep_names.contains(&"libfoo"),
            "libfoo should be in host dependencies when host_platform is osx-arm64, \
             got: {host_dep_names:?}"
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
                Item::Value(Value::Template(s)) if s.contains("stdlib") => Some(s.clone()),
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
