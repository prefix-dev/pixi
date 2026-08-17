mod build_script;
mod config;

use build_script::BuildScriptContext;
use config::{MojoBackendConfig, MojoPackageFormat, clean_project_name};
use miette::{Error, IntoDiagnostic};
use pixi_build_backend::generated_recipe::DefaultMetadataProvider;
use pixi_build_backend::{
    compilers::default_compiler_variants,
    generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    tools::BackendIdentifier,
};
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{Item, JinjaTemplate, Script, SerializableMatchSpec, Value};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::{ChannelUrl, Flag, NoArchType, Platform};
use std::collections::HashSet;
use std::path::PathBuf;
use std::{collections::BTreeMap, path::Path, sync::Arc};

#[derive(Default, Clone)]
pub struct MojoGenerator {}

#[async_trait::async_trait]
impl GenerateRecipe for MojoGenerator {
    type Config = MojoBackendConfig;

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

        let mut generated_recipe =
            GeneratedRecipe::from_model(model.clone(), &mut DefaultMetadataProvider)
                .into_diagnostic()?;

        let cleaned_project_name = clean_project_name(
            generated_recipe
                .recipe
                .package
                .name
                .as_concrete()
                .ok_or(Error::msg("Package is missing a name"))?
                .as_str(),
        );

        // Auto-derive bins and pkg fields/configs if needed
        let (mut bins, mut pkg) = config.auto_derive(&manifest_root, &cleaned_project_name)?;
        Self::make_paths_absolute(&manifest_root, &mut bins, &mut pkg)?;

        let has_package = pkg.is_some();
        let package_format = pkg.as_ref().and_then(|pkg| pkg.format);
        if has_package && package_format != Some(MojoPackageFormat::Precompiled) && bins.is_some() {
            miette::bail!(
                "Mojo source packages cannot contain compiled binaries; set `pkg.format = \"precompiled\"` or omit `bins`"
            );
        }

        if has_package {
            let flag = match package_format {
                Some(MojoPackageFormat::Source) => {
                    Value::new_concrete("mojo:source".parse::<Flag>().unwrap(), None)
                }
                Some(MojoPackageFormat::Precompiled) => {
                    Value::new_concrete("mojo:precompiled".parse::<Flag>().unwrap(), None)
                }
                None => Value::new_template(
                    JinjaTemplate::new(
                        "${{ 'mojo:source' if mojo_package_format == 'source' else 'mojo:precompiled' }}".to_string(),
                    )
                    .expect("static Mojo format flag expression is valid"),
                    None,
                ),
            };
            generated_recipe.recipe.build.flags.push(Item::Value(flag));

            // Mojo precompiled packages contain non-elaborated code and are also
            // architecture-independent, so both package formats are noarch.
            generated_recipe.recipe.build.noarch =
                Some(Value::new_concrete(NoArchType::generic(), None));

            if package_format.is_none() {
                generated_recipe
                    .recipe
                    .build
                    .variant
                    .down_prioritize_variant = Some(Value::new_template(
                    JinjaTemplate::new(
                        "${{ 1 if mojo_package_format == 'precompiled' else 0 }}".to_string(),
                    )
                    .expect("static Mojo variant priority expression is valid"),
                    None,
                ));
            }
        }

        // Add compiler
        let requirements = &mut generated_recipe.recipe.requirements;

        let compilers = config.compilers.clone().unwrap_or_default();

        pixi_build_backend::compilers::add_compilers_to_requirements(
            &compilers,
            &mut requirements.build,
        );
        pixi_build_backend::compilers::add_stdlib_to_requirements(
            &compilers,
            &mut requirements.build,
            variants,
        );

        if has_package && package_format != Some(MojoPackageFormat::Source) {
            let (compiler_requirement, exact_compiler_pin) = match package_format {
                Some(MojoPackageFormat::Precompiled) => (
                    "mojo-compiler",
                    "${{ pin_compatible('mojo-compiler', exact=true) }}",
                ),
                None => (
                    "${{ 'mojo-compiler' if mojo_package_format == 'precompiled' }}",
                    "${{ pin_compatible('mojo-compiler', exact=true) if mojo_package_format == 'precompiled' }}",
                ),
                Some(MojoPackageFormat::Source) => unreachable!(),
            };
            let compiler_requirement = Value::new_template(
                JinjaTemplate::new(compiler_requirement.to_string())
                    .expect("static Mojo compiler requirement is valid"),
                None,
            );
            requirements
                .build
                .push(Item::Value(compiler_requirement.clone()));
            requirements.host.push(Item::Value(compiler_requirement));
            requirements
                .run
                .push(Item::<SerializableMatchSpec>::Value(Value::new_template(
                    JinjaTemplate::new(exact_compiler_pin.to_string())
                        .expect("static pin_compatible expression is valid"),
                    None,
                )));
        }

        let pkg_format = match package_format {
            Some(MojoPackageFormat::Source) => "source",
            Some(MojoPackageFormat::Precompiled) => "precompiled",
            None => "${{ mojo_package_format }}",
        }
        .to_string();
        let build_script = BuildScriptContext {
            bins,
            pkg,
            pkg_format,
            is_windows: host_platform.is_windows(),
        }
        .render();

        generated_recipe.recipe.build.script = Script::from_content(build_script)
            .with_env(
                config
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::new_concrete(v.clone(), None)))
                    .collect(),
            )
            .with_secrets(model.secrets.iter().cloned().collect());

        generated_recipe.build_input_globs = Self::globs().collect();

        Ok(generated_recipe)
    }

    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<Vec<String>> {
        Ok(Self::globs()
            .chain(config.extra_input_globs.clone())
            .collect())
    }

    fn default_variants(
        &self,
        host_platform: Platform,
    ) -> miette::Result<BTreeMap<NormalizedKey, Vec<Variable>>> {
        let mut variants = default_compiler_variants(host_platform);
        variants.insert(
            NormalizedKey::from("mojo_package_format"),
            vec!["source".into(), "precompiled".into()],
        );
        Ok(variants)
    }
}

impl MojoGenerator {
    fn make_paths_absolute(
        manifest_root: &Path,
        bins: &mut Option<Vec<config::MojoBinConfig>>,
        pkg: &mut Option<config::MojoPkgConfig>,
    ) -> miette::Result<()> {
        if let Some(bins) = bins {
            for bin in bins {
                if let Some(path) = &mut bin.path {
                    *path = Self::absolute_path(manifest_root, path)?;
                }
            }
        }
        if let Some(pkg) = pkg
            && let Some(path) = &mut pkg.path
        {
            *path = Self::absolute_path(manifest_root, path)?;
        }
        Ok(())
    }

    fn absolute_path(manifest_root: &Path, path: &str) -> miette::Result<String> {
        let path = Path::new(path);
        if path.is_absolute() {
            return Ok(path.display().to_string());
        }
        let manifest_root = if manifest_root.is_absolute() {
            manifest_root.to_path_buf()
        } else {
            std::env::current_dir()
                .into_diagnostic()?
                .join(manifest_root)
        };
        Ok(manifest_root.join(path).display().to_string())
    }

    fn globs() -> impl Iterator<Item = String> {
        [
            // Source files
            "**/*.mojo",
        ]
        .iter()
        .map(|s: &&str| s.to_string())
    }
}

#[tokio::main]
pub async fn main() {
    if let Err(err) = pixi_build_backend::cli::main(|log| {
        IntermediateBackendInstantiator::<MojoGenerator>::new(
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
    use std::path::PathBuf;

    use super::*;
    use crate::config::{MojoBinConfig, MojoPkgConfig};
    use fs_err as fs;
    use indexmap::IndexMap;

    #[test]
    fn test_input_globs_includes_extra_globs() {
        let config = MojoBackendConfig {
            extra_input_globs: vec![String::from("**/.c")],
            ..Default::default()
        };

        let generator = MojoGenerator::default();

        let result = generator.extract_input_globs_from_build(&config, PathBuf::new(), false);

        insta::assert_debug_snapshot!(result);
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
    async fn test_mojo_bin_is_set() {
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

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    bins: Some(vec![MojoBinConfig {
                        name: Some(String::from("example")),
                        path: Some(String::from("./main.mojo")),
                        extra_args: Some(vec![String::from("-I"), String::from(".")]),
                    }]),
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

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_mojo_pkg_is_set() {
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

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    bins: Some(vec![MojoBinConfig {
                        name: Some(String::from("example")),
                        path: Some(String::from("./main.mojo")),
                        extra_args: Some(vec![String::from("-i"), String::from(".")]),
                    }]),
                    pkg: Some(MojoPkgConfig {
                        name: Some(String::from("lib")),
                        format: Some(MojoPackageFormat::Precompiled),
                        path: Some(String::from("mylib")),
                        extra_args: Some(vec![String::from("-i"), String::from(".")]),
                    }),
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

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
        ".source[0].path" => "[ ... path ... ]",
        ".build.script" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn source_package_is_noarch_and_copies_sources() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });
        let temp = tempfile::TempDir::new().unwrap();
        let package_dir = temp.path().join("foobar");
        fs::create_dir_all(&package_dir).unwrap();
        fs::write(package_dir.join("__init__.mojo"), "").unwrap();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    pkg: Some(MojoPkgConfig {
                        format: Some(MojoPackageFormat::Source),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                temp.path().to_path_buf(),
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
            .unwrap();

        assert_eq!(
            generated_recipe
                .recipe
                .build
                .noarch
                .as_ref()
                .and_then(Value::as_concrete),
            Some(&NoArchType::generic())
        );
        assert_eq!(
            generated_recipe
                .recipe
                .build
                .flags
                .iter()
                .next()
                .and_then(Item::as_value)
                .and_then(Value::as_concrete)
                .map(Flag::as_str),
            Some("mojo:source")
        );
        let script_content = generated_recipe.recipe.build.script.content.unwrap();
        let script = script_content
            .iter()
            .next()
            .and_then(Item::as_value)
            .and_then(Value::as_concrete)
            .unwrap();
        assert!(script.contains("cp -R"), "source build script:\n{script}");
        assert!(
            script.contains("lib/mojo/foobar"),
            "source build script:\n{script}"
        );
        assert!(
            !script.contains("mojo --version"),
            "source build script should not require Mojo:\n{script}"
        );
    }

    #[test]
    fn default_variants_offer_source_before_precompiled() {
        let variants = MojoGenerator::default()
            .default_variants(Platform::Linux64)
            .unwrap();
        let formats = variants
            .get(&NormalizedKey::from("mojo_package_format"))
            .unwrap()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();

        assert_eq!(formats, ["source", "precompiled"]);
    }

    #[tokio::test]
    async fn source_package_rejects_compiled_binaries() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let result = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    bins: Some(vec![MojoBinConfig {
                        name: Some("foobar".to_string()),
                        path: Some("main.mojo".to_string()),
                        extra_args: None,
                    }]),
                    pkg: Some(MojoPkgConfig {
                        name: Some("foobar".to_string()),
                        path: Some("foobar".to_string()),
                        ..Default::default()
                    }),
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
            .await;

        let error = match result {
            Ok(_) => panic!("source package with a binary should fail"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("cannot contain compiled binaries")
        );
    }

    #[tokio::test]
    async fn test_relative_paths_are_made_absolute() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });
        let temp = tempfile::TempDir::new().unwrap();
        let source_dir = temp.path().to_path_buf();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    bins: Some(vec![MojoBinConfig {
                        name: Some(String::from("example")),
                        path: Some(String::from("src/main.mojo")),
                        extra_args: None,
                    }]),
                    pkg: Some(MojoPkgConfig {
                        name: Some(String::from("lib")),
                        format: Some(MojoPackageFormat::Precompiled),
                        path: Some(String::from("src/foobar")),
                        extra_args: None,
                    }),
                    ..Default::default()
                },
                source_dir.clone(),
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

        let content = generated_recipe.recipe.build.script.content.unwrap();
        let script = content
            .iter()
            .next()
            .unwrap()
            .as_value()
            .unwrap()
            .as_concrete()
            .unwrap();
        let bin_path = source_dir.join("src/main.mojo").display().to_string();
        let pkg_path = source_dir.join("src/foobar").display().to_string();

        assert!(
            !script
                .lines()
                .any(|line| line.trim_start().starts_with("cd ")),
            "script should not change directory:\n{script}"
        );
        assert!(
            script.contains(&format!("\"{bin_path}\"")),
            "script should use absolute bin path:\n{script}"
        );
        assert!(
            script.contains(&format!("\"{pkg_path}\"")),
            "script should use absolute pkg path:\n{script}"
        );
    }

    #[tokio::test]
    async fn test_compiler_is_in_build_requirements() {
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

        // Create a temporary directory with a main.mojo file so the test has something to build
        let temp = tempfile::TempDir::new().unwrap();
        fs::write(temp.path().join("main.mojo"), "def main():\n    pass").unwrap();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig::default(),
                temp.path().to_path_buf(),
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

        // Create a temporary directory with a main.mojo file so the test has something to build
        let temp = tempfile::TempDir::new().unwrap();
        fs::write(temp.path().join("main.mojo"), "def main():\n    pass").unwrap();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    env: env.clone(),
                    ..Default::default()
                },
                temp.path().to_path_buf(),
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

        insta::assert_yaml_snapshot!(generated_recipe.recipe.build.script,
        {
            ".content" => "[ ... script ... ]",
        });
    }

    #[tokio::test]
    async fn test_compiler_is_not_added_if_compiler_is_already_present() {
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
                        "mojo-compiler": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                },
            }
        });

        // Create a temporary directory with a main.mojo file so the test has something to build
        let temp = tempfile::TempDir::new().unwrap();
        fs::write(temp.path().join("main.mojo"), "def main():\n    pass").unwrap();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig::default(),
                temp.path().to_path_buf(),
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
    async fn test_mojo_with_additional_compilers() {
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

        // Create a temporary directory with a main.mojo file so the test has something to build
        let temp = tempfile::TempDir::new().unwrap();
        fs::write(temp.path().join("main.mojo"), "def main():\n    pass").unwrap();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    compilers: Some(vec!["c".to_string(), "cxx".to_string()]),
                    ..Default::default()
                },
                temp.path().to_path_buf(),
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

        // Check that we have both the mojo-compiler package and the additional compilers
        let build_reqs = &generated_recipe.recipe.requirements.build;

        // Check for additional compiler templates
        let compiler_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(v) => {
                    let s = v.as_template()?.to_string();
                    s.contains("compiler").then_some(s)
                }
                _ => None,
            })
            .collect();

        // Should have exactly two additional compilers (c and cxx, but not mojo template)
        assert_eq!(
            compiler_templates.len(),
            2,
            "Should have exactly two additional compilers"
        );

        // Check we have the expected additional compilers
        assert!(
            compiler_templates.contains(&"${{ compiler('c') }}".to_string()),
            "C compiler should be in build requirements"
        );
        assert!(
            compiler_templates.contains(&"${{ compiler('cxx') }}".to_string()),
            "C++ compiler should be in build requirements"
        );

        // Ensure we don't have a mojo template (since mojo uses special package)
        assert!(
            !compiler_templates.contains(&"${{ compiler('mojo') }}".to_string()),
            "Should not have mojo compiler template since it uses special package"
        );
    }

    #[tokio::test]
    async fn test_default_mojo_compiler_behavior() {
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

        // Create a temporary directory with a main.mojo file so the test has something to build
        let temp = tempfile::TempDir::new().unwrap();
        fs::write(temp.path().join("main.mojo"), "def main():\n    pass").unwrap();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    compilers: None,
                    ..Default::default()
                },
                temp.path().to_path_buf(),
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

        // Check that we have only the mojo-compiler package by default
        let build_reqs = &generated_recipe.recipe.requirements.build;

        // Check that no additional compiler templates are present
        let compiler_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(v) => {
                    let s = v.as_template()?.to_string();
                    s.contains("compiler").then_some(s)
                }
                _ => None,
            })
            .collect();

        // Should have no additional compiler templates by default
        assert_eq!(
            compiler_templates.len(),
            0,
            "Should have no additional compiler templates by default"
        );
    }

    #[tokio::test]
    async fn test_opt_out_of_mojo_compiler() {
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

        // Create a temporary directory with a main.mojo file so the test has something to build
        let temp = tempfile::TempDir::new().unwrap();
        fs::write(temp.path().join("main.mojo"), "def main():\n    pass").unwrap();

        let generated_recipe = MojoGenerator::default()
            .generate_recipe(
                &project_model,
                &MojoBackendConfig {
                    compilers: Some(vec!["c".to_string(), "cxx".to_string()]),
                    ..Default::default()
                },
                temp.path().to_path_buf(),
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

        // Check that mojo-compiler is NOT present when user opts out
        let build_reqs = &generated_recipe.recipe.requirements.build;

        // Check for mojo-compiler package (should NOT be present)
        let has_mojo_compiler = build_reqs
            .iter()
            .any(|item| format!("{item:?}").contains("mojo-compiler"));
        assert!(
            !has_mojo_compiler,
            "Should NOT have mojo-compiler package when user opts out"
        );

        // Check for other compiler templates
        let compiler_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(v) => {
                    let s = v.as_template()?.to_string();
                    s.contains("compiler").then_some(s)
                }
                _ => None,
            })
            .collect();

        // Should have exactly two compilers (c and cxx)
        assert_eq!(
            compiler_templates.len(),
            2,
            "Should have exactly two compilers when opting out of mojo"
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
    }
}
