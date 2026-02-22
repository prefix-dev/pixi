mod build_script;
mod config;
mod metadata;

use build_script::BuildScriptContext;
use config::RustBackendConfig;
use metadata::CargoMetadataProvider;
use miette::IntoDiagnostic;
use pixi_build_backend::variants::NormalizedKey;
use pixi_build_backend::{
    Variable,
    cache::{sccache_envs, sccache_tools},
    generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    traits::ProjectModel,
};
use rattler_conda_types::{ChannelUrl, Platform};
use recipe_stage0::{
    matchspec::PackageDependency,
    recipe::{Item, Script},
};
use std::collections::HashSet;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default, Clone)]
pub struct RustGenerator {}

#[async_trait::async_trait]
impl GenerateRecipe for RustGenerator {
    type Config = RustBackendConfig;

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
        // Construct a CargoMetadataProvider to read the Cargo.toml file
        // and extract metadata from it.
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

        let mut cargo_metadata = CargoMetadataProvider::new(
            &manifest_root,
            config.ignore_cargo_manifest.is_some_and(|ignore| ignore),
        );

        // Create the recipe
        let mut generated_recipe =
            GeneratedRecipe::from_model(model.clone(), &mut cargo_metadata).into_diagnostic()?;

        // we need to add compilers
        let requirements = &mut generated_recipe.recipe.requirements;

        // Get the platform-specific dependencies from the project model.
        // This properly handles target selectors like [target.linux-64] by using
        // the ProjectModel trait's platform-aware API instead of trying to evaluate
        // rattler-build selectors with simple string comparison.
        let model_dependencies = model.dependencies(Some(host_platform));

        // Get the list of compilers from config, defaulting to ["rust", "c"] if not
        // specified. The rust compilers already depend on the c compiler.
        // Adding it here allows to version the c compiler through the variant `c_compiler_version`.
        let compilers = config
            .compilers
            .clone()
            .unwrap_or_else(|| vec!["rust".to_string(), "c".to_string()]);

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

        // Check if openssl is in the host dependencies
        let has_openssl = model_dependencies
            .host
            .contains_key(&pixi_build_types::SourcePackageName::from("openssl"));

        let mut has_sccache = false;

        let config_env = config.env.clone();

        let system_env_vars = config
            .system_env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>();

        let all_env_vars = config_env
            .clone()
            .into_iter()
            .chain(system_env_vars.clone())
            .collect();

        let mut sccache_secrets = Vec::default();

        // Verify if user has set any sccache environment variables
        if sccache_envs(&all_env_vars).is_some() {
            // check if we set some sccache in system env vars
            if let Some(system_sccache_keys) = sccache_envs(&system_env_vars) {
                // If sccache_envs are used in the system environment variables,
                // we need to set them as secrets
                let system_sccache_keys = system_env_vars
                    .keys()
                    // we set only those keys that are present in the system environment variables
                    // and not in the config env
                    .filter(|key| {
                        system_sccache_keys.contains(&key.as_str())
                            && !config_env.contains_key(*key)
                    })
                    .cloned()
                    .collect();

                sccache_secrets = system_sccache_keys;
            };

            let sccache_dep: Vec<Item<PackageDependency>> = sccache_tools()
                .iter()
                .map(|tool| tool.parse().into_diagnostic())
                .collect::<miette::Result<Vec<_>>>()?;

            // Add sccache tools to the build requirements
            // only if they are not already present
            let existing_reqs: Vec<_> = requirements.build.clone().into_iter().collect();

            requirements.build.extend(
                sccache_dep
                    .into_iter()
                    .filter(|dep| !existing_reqs.contains(dep)),
            );

            has_sccache = true;
        }

        // Synthesize cargo_args: add --bin for each binary if specified
        let mut cargo_args = config.extra_args.clone();
        for bin in &config.binaries {
            cargo_args.push("--bin".to_string());
            cargo_args.push(bin.clone());
        }

        let build_script = BuildScriptContext {
            source_dir: manifest_root.display().to_string(),
            extra_args: cargo_args,
            has_openssl,
            has_sccache,
            is_bash: !Platform::current().is_windows(),
        }
        .render();

        generated_recipe.recipe.build.script = Script {
            content: build_script,
            env: config_env,
            secrets: sccache_secrets,
        };

        // Add the input globs from the Cargo metadata provider
        generated_recipe
            .metadata_input_globs
            .extend(cargo_metadata.input_globs());

        Ok(generated_recipe)
    }

    /// Returns the build input globs used by the backend.
    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<BTreeSet<String>> {
        Ok([
            "**/*.rs",
            // Cargo configuration files
            "Cargo.toml",
            "Cargo.lock",
            // Build scripts
            "build.rs",
        ]
        .iter()
        .map(|s| s.to_string())
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
        IntermediateBackendInstantiator::<RustGenerator>::new(log, Arc::default())
    })
    .await
    {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {

    #[tokio::test]
    async fn test_binaries_flag_is_rendered() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig {
                    binaries: vec!["rattler-build".to_string()],
                    ignore_cargo_manifest: Some(true),
                    ..Default::default()
                },
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &std::collections::HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        let content = &generated_recipe.recipe.build.script.content;
        assert!(content.contains("--bin rattler-build"));
    }
    use cargo_toml::Manifest;
    use indexmap::IndexMap;
    use recipe_stage0::recipe::{Item, Value};

    use super::*;

    #[test]
    fn test_input_globs_includes_extra_globs() {
        let config = RustBackendConfig {
            extra_input_globs: vec!["custom/*.txt".to_string(), "extra/**/*.py".to_string()],
            ..Default::default()
        };

        let generator = RustGenerator::default();

        let result = generator
            .extract_input_globs_from_build(&config, PathBuf::new(), false)
            .unwrap();

        // Verify that all extra globs are included in the result
        for extra_glob in &config.extra_input_globs {
            assert!(
                result.contains(extra_glob),
                "Result should contain extra glob: {extra_glob}"
            );
        }

        // Verify that default globs are still present
        assert!(result.contains("**/*.rs"));
        assert!(result.contains("Cargo.toml"));
        assert!(result.contains("Cargo.lock"));
        assert!(result.contains("build.rs"));
    }

    #[macro_export]
    macro_rules! project_fixture {
        ($($json:tt)+) => {
            serde_json::from_value::<pixi_build_types::ProjectModel>(
                serde_json::json!($($json)+)
            ).expect("Failed to create TestProjectModel from JSON fixture.")
        };
    }

    #[tokio::test]
    async fn test_rust_is_in_build_requirements() {
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

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig::default_with_ignore_cargo_manifest(),
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
    async fn test_rust_is_not_added_if_already_present() {
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
                    },
                    "buildDependencies": {
                        "rust": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig::default_with_ignore_cargo_manifest(),
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

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig {
                    env: env.clone(),
                    system_env: Default::default(),
                    ignore_cargo_manifest: Some(true),
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
    async fn test_sccache_is_enabled() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "default_target": {
                    "run_dependencies": {
                        "boltons": "*"
                    }
                },
            }
        });

        let env = IndexMap::from([("SCCACHE_BUCKET".to_string(), "my-bucket".to_string())]);
        let system_env = IndexMap::from([
            ("SCCACHE_SYSTEM".to_string(), "SOME_VALUE".to_string()),
            ("SCCACHE_BUCKET".to_string(), "system-bucket".to_string()),
        ]);

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig {
                    env,
                    system_env,
                    ignore_cargo_manifest: Some(true),
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

        // Clean up environment variables
        // SAFETY: We're in a test and cleaning up the environment after the test
        unsafe {
            std::env::remove_var("SCCACHE_SYSTEM");
            std::env::remove_var("SCCACHE_BUCKET");
        }

        // Verify that sccache is added to the build requirements
        // when some env variables are set
        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script.content" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_with_cargo_manifest() {
        let project_model = project_fixture!({});

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig::default(),
                // Using this crate itself, as it has interesting metadata, using .workspace
                std::env::current_dir().unwrap(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Manually load the Cargo manifest to ensure it works
        let current_dir = std::env::current_dir().unwrap();
        let package_manifest_path = current_dir.join("Cargo.toml");
        let mut manifest = Manifest::from_path(&package_manifest_path).unwrap();
        manifest.complete_from_path(&package_manifest_path).unwrap();

        assert_eq!(
            manifest.clone().package.unwrap().name.clone(),
            generated_recipe.recipe.package.name.to_string()
        );
        assert_eq!(
            *manifest.clone().package.unwrap().version.get().unwrap(),
            generated_recipe.recipe.package.version.to_string()
        );
        assert_eq!(
            *manifest
                .clone()
                .package
                .unwrap()
                .description
                .unwrap()
                .get()
                .unwrap(),
            generated_recipe
                .recipe
                .about
                .as_ref()
                .and_then(|a| a.description.clone())
                .unwrap()
                .to_string()
        );
        assert_eq!(
            *manifest
                .clone()
                .package
                .unwrap()
                .license
                .unwrap()
                .get()
                .unwrap(),
            generated_recipe
                .recipe
                .about
                .as_ref()
                .and_then(|a| a.license.clone())
                .unwrap()
                .to_string()
        );
        assert_eq!(
            *manifest
                .clone()
                .package
                .unwrap()
                .repository
                .unwrap()
                .get()
                .unwrap(),
            generated_recipe
                .recipe
                .about
                .as_ref()
                .and_then(|a| a.repository.clone())
                .unwrap()
                .to_string()
        );

        insta::assert_yaml_snapshot!(&generated_recipe.metadata_input_globs, @r###"
        - "../../Cargo.toml"
        - "../Cargo.toml"
        - Cargo.toml
        "###);
    }

    #[tokio::test]
    async fn test_error_handling_missing_cargo_manifest() {
        let project_model = project_fixture!({
            "targets": {
                "default_target": {
                    "run_dependencies": {
                        "dependency": "*"
                    }
                },
            }
        });

        // Try to generate recipe from a non-existent directory
        let result = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig::default(),
                PathBuf::from("/non/existent/path"),
                Platform::Linux64,
                None,
                &std::collections::HashSet::new(),
                vec![],
                None,
            )
            .await;

        // Should fail when trying to read Cargo.toml from non-existent path
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_empty_name() {
        let project_model = project_fixture!({
            "version": "0.1.0",
            "targets": {
                "default_target": {
                    "run_dependencies": {
                        "dependency": "*"
                    }
                },
            }
        });

        // Should fail because name is empty and we're ignoring cargo manifest
        let result = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig::default_with_ignore_cargo_manifest(),
                std::env::current_dir().unwrap(),
                Platform::Linux64,
                None,
                &std::collections::HashSet::new(),
                vec![],
                None,
            )
            .await;

        assert!(result.is_err());
        let error_message = result.err().unwrap().to_string();
        assert!(error_message.contains("no name defined"));
    }

    #[tokio::test]
    async fn test_multiple_compilers_configuration() {
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

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig {
                    compilers: Some(vec!["rust".to_string(), "c".to_string(), "cxx".to_string()]),
                    ignore_cargo_manifest: Some(true),
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
            compiler_templates.contains(&"${{ compiler('rust') }}".to_string()),
            "Rust compiler should be in build requirements"
        );
        assert!(
            compiler_templates.contains(&"${{ compiler('c') }}".to_string()),
            "C compiler should be in build requirements"
        );
        assert!(
            compiler_templates.contains(&"${{ compiler('cxx') }}".to_string()),
            "C++ compiler should be in build requirements"
        );
    }

    #[tokio::test]
    async fn test_default_compiler_when_not_specified() {
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

        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig {
                    compilers: None,
                    ignore_cargo_manifest: Some(true),
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

        // Check that we have exactly the expected compilers and build tools
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let compiler_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(Value::Template(s)) if s.contains("compiler") => Some(s.clone()),
                _ => None,
            })
            .collect();

        // Should have exactly two compilers: rust and c
        assert_eq!(
            compiler_templates.len(),
            2,
            "Should have exactly two compilers when not specified"
        );
        assert!(
            compiler_templates.contains(&"${{ compiler('rust') }}".to_string()),
            "Default compilers should include rust"
        );
        assert!(
            compiler_templates.contains(&"${{ compiler('c') }}".to_string()),
            "Default compilers should include c"
        );
    }

    #[tokio::test]
    async fn test_target_specific_build_dependencies_linux() {
        use pixi_build_backend::traits::ProjectModel;

        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "targets": {
                    "linux-64": {
                        "buildDependencies": {
                            "openssl": {
                                "binary": {
                                    "version": ">=3.0"
                                }
                            }
                        }
                    }
                }
            }
        });

        // Test that the ProjectModel correctly filters dependencies for Linux64
        let linux_deps = project_model.dependencies(Some(Platform::Linux64));
        assert!(
            linux_deps
                .build
                .contains_key(&pixi_build_types::SourcePackageName::from("openssl")),
            "openssl should be in build dependencies for Linux64"
        );

        // Test that the ProjectModel correctly excludes dependencies for Osx64
        let osx_deps = project_model.dependencies(Some(Platform::Osx64));
        assert!(
            !osx_deps
                .build
                .contains_key(&pixi_build_types::SourcePackageName::from("openssl")),
            "openssl should NOT be in build dependencies for Osx64"
        );

        // Test that the intermediate recipe contains the conditional items with correct condition
        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig::default_with_ignore_cargo_manifest(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Verify that conditional build dependencies contain openssl with linux-64 condition
        let mut found_openssl_conditional = false;
        for item in &generated_recipe.recipe.requirements.build {
            if let Item::Conditional(cond) = item {
                // Check if the then branch contains openssl
                if cond
                    .then
                    .0
                    .iter()
                    .any(|dep| dep.package_name().as_source() == "openssl")
                {
                    // Print the actual condition for debugging
                    eprintln!(
                        "Found openssl conditional with condition: '{}'",
                        cond.condition
                    );
                    // The condition should be exactly "host_platform == 'linux-64'"
                    assert_eq!(
                        cond.condition, "host_platform == 'linux-64'",
                        "Condition should be exactly \"host_platform == 'linux-64'\""
                    );
                    found_openssl_conditional = true;
                    break;
                }
            }
        }

        assert!(
            found_openssl_conditional,
            "Recipe should contain conditional build dependency for openssl with linux-64 condition"
        );
    }

    #[tokio::test]
    async fn test_target_specific_build_dependencies_with_unix_selector() {
        use pixi_build_backend::traits::ProjectModel;

        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "targets": {
                    "unix": {
                        "buildDependencies": {
                            "gcc": {
                                "binary": {
                                    "version": "*"
                                }
                            }
                        }
                    }
                }
            }
        });

        // Test that the ProjectModel correctly filters dependencies for Linux64 (unix)
        let linux_deps = project_model.dependencies(Some(Platform::Linux64));
        assert!(
            linux_deps
                .build
                .contains_key(&pixi_build_types::SourcePackageName::from("gcc")),
            "gcc should be in build dependencies for Linux64 (unix)"
        );

        // Test that the ProjectModel correctly filters dependencies for Osx64 (unix)
        let osx_deps = project_model.dependencies(Some(Platform::Osx64));
        assert!(
            osx_deps
                .build
                .contains_key(&pixi_build_types::SourcePackageName::from("gcc")),
            "gcc should be in build dependencies for Osx64 (unix)"
        );

        // Test that the ProjectModel correctly excludes dependencies for Win64 (not unix)
        let win_deps = project_model.dependencies(Some(Platform::Win64));
        assert!(
            !win_deps
                .build
                .contains_key(&pixi_build_types::SourcePackageName::from("gcc")),
            "gcc should NOT be in build dependencies for Win64 (not unix)"
        );

        // Test that the intermediate recipe contains the conditional items with correct condition
        let generated_recipe = RustGenerator::default()
            .generate_recipe(
                &project_model,
                &RustBackendConfig::default_with_ignore_cargo_manifest(),
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Verify that conditional build dependencies contain gcc with unix condition
        let mut found_gcc_conditional = false;
        for item in &generated_recipe.recipe.requirements.build {
            if let Item::Conditional(cond) = item {
                // Check if the then branch contains gcc
                if cond
                    .then
                    .0
                    .iter()
                    .any(|dep| dep.package_name().as_source() == "gcc")
                {
                    // Print the actual condition for debugging
                    eprintln!("Found gcc conditional with condition: '{}'", cond.condition);
                    // The condition should be exactly "unix"
                    assert_eq!(
                        cond.condition, "unix",
                        "Condition should be exactly \"unix\""
                    );
                    found_gcc_conditional = true;
                    break;
                }
            }
        }

        assert!(
            found_gcc_conditional,
            "Recipe should contain conditional build dependency for gcc with unix condition"
        );
    }
}
