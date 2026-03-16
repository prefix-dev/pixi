mod build_script;
mod config;
mod metadata;
mod pypi_mapping;

use build_script::{BuildPlatform, BuildScriptContext, Installer};
use config::PythonBackendConfig;
use fs_err as fs;
use miette::IntoDiagnostic;
use pixi_build_backend::variants::NormalizedKey;
use pixi_build_backend::{
    Variable,
    generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    traits::ProjectModel,
};
use pyproject_toml::PyProjectToml;
use rattler_conda_types::{ChannelUrl, Platform, Version, VersionBumpType, package::EntryPoint};
use recipe_stage0::matchspec::PackageDependency;
use recipe_stage0::recipe::{Item, NoArchKind, Python, Script};
use std::collections::HashSet;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use crate::metadata::{PyprojectManifestMode, PyprojectMetadataProvider};
use crate::pypi_mapping::{
    detect_compilers_from_build_requirements, filter_mapped_pypi_deps,
    map_requirements_with_channels,
};

/// Compute the `python_abi` version spec from an optional `requires-python`
/// specifier string.
///
/// Extracts the lower bound (first `>=` specifier) and pins it to a single
/// minor version:
/// - `">=3.9"`     → `">=3.9,<3.10.0a0"`
/// - `">=3.9.3"`   → `">=3.9.3,<3.10.0a0"`
/// - `">=3.11,<4"` → `">=3.11,<3.12.0a0"`
/// - `None`        → `">=3.8,<3.9.0a0"` (default)
fn python_abi_spec_from_requires_python(requires_python: Option<&str>) -> miette::Result<String> {
    let lower_bound = requires_python
        .and_then(|s| {
            let specifiers = pep440_rs::VersionSpecifiers::from_str(s).ok()?;
            specifiers
                .iter()
                .find(|spec| *spec.operator() == pep440_rs::Operator::GreaterThanEqual)
                .map(|spec| {
                    let pep_version = spec.version();
                    // Convert pep440 version to rattler Version via string round-trip
                    Version::from_str(&pep_version.to_string())
                        .expect("pep440 version should be a valid conda version")
                })
        })
        .unwrap_or_else(|| Version::from_str("3.8").expect("valid version"));

    // Truncate to major.minor for the upper bound computation
    let major_minor = lower_bound
        .clone()
        .with_segments(..std::cmp::min(lower_bound.segment_count(), 2))
        .ok_or_else(|| miette::miette!("failed to truncate version to major.minor"))?;

    let upper_bound = major_minor
        .bump(VersionBumpType::Minor)
        .into_diagnostic()?
        .with_alpha()
        .remove_local()
        .into_owned();

    Ok(format!(">={lower_bound},<{upper_bound}"))
}

#[derive(Default, Clone)]
pub struct PythonGenerator {}

impl PythonGenerator {
    /// Read the entry points from the pyproject.toml and return them as a list.
    ///
    /// If the manifest is not a pyproject.toml file no entry-points are added.
    pub(crate) fn entry_points(pyproject_manifest: Option<PyProjectToml>) -> Vec<EntryPoint> {
        let scripts = pyproject_manifest
            .as_ref()
            .and_then(|p| p.project.as_ref())
            .and_then(|p| p.scripts.as_ref());

        scripts
            .into_iter()
            .flatten()
            .flat_map(|(name, entry_point)| {
                EntryPoint::from_str(&format!("{name} = {entry_point}"))
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl GenerateRecipe for PythonGenerator {
    type Config = PythonBackendConfig;

    async fn generate_recipe(
        &self,
        model: &pixi_build_types::ProjectModel,
        config: &Self::Config,
        manifest_path: PathBuf,
        host_platform: Platform,
        python_params: Option<PythonParams>,
        variants: &HashSet<NormalizedKey>,
        channels: Vec<ChannelUrl>,
        cache_dir: Option<PathBuf>,
    ) -> miette::Result<GeneratedRecipe> {
        let params = python_params.unwrap_or_default();

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

        let mode = if config
            .ignore_pyproject_manifest
            .is_some_and(|ignore| ignore)
        {
            PyprojectManifestMode::Ignore
        } else {
            PyprojectManifestMode::Read
        };
        let mut pyproject_metadata_provider = PyprojectMetadataProvider::new(&manifest_root, mode);

        let mut generated_recipe =
            GeneratedRecipe::from_model(model.clone(), &mut pyproject_metadata_provider)
                .into_diagnostic()?;

        let requirements = &mut generated_recipe.recipe.requirements;

        // Get the platform-specific dependencies from the project model.
        // This properly handles target selectors like [target.linux-64] by using
        // the ProjectModel trait's platform-aware API instead of trying to evaluate
        // rattler-build selectors with simple string comparison.
        let model_dependencies = model.dependencies(Some(host_platform));

        // Ensure the python build tools are added to the `host` requirements.
        // Please note: this is a subtle difference for python, where the build tools
        // are added to the `host` requirements, while for cmake/rust they are
        // added to the `build` requirements.
        // We only check build and host dependencies for the installer.
        let installer =
            Installer::determine_installer_from_names(model_dependencies.build_and_host_names());

        let installer_name = installer.package_name().to_string();
        let installer_pkg = pixi_build_types::SourcePackageName::from(installer_name.as_str());

        // add installer in the host requirements
        if !model_dependencies.host.contains_key(&installer_pkg) {
            requirements
                .host
                .push(installer_name.parse().into_diagnostic()?);
        }

        // Get Python requirement spec
        let python_requirement_str = match pyproject_metadata_provider.requires_python() {
            Ok(Some(requires_python)) => format!("python {requires_python}"),
            _ => "python".to_string(),
        };

        // Add python to host and run requirements, if not already set in the package manifest
        let python_pkg = pixi_build_types::SourcePackageName::from("python");
        let python_requirement: Item<PackageDependency> =
            python_requirement_str.parse().into_diagnostic()?;
        if !model_dependencies.host.contains_key(&python_pkg) {
            requirements.host.push(python_requirement.clone());
        }
        if !model_dependencies.run.contains_key(&python_pkg) {
            requirements.run.push(python_requirement);
        }

        // Detect compilers from build-system.requires (e.g., maturin -> rust)
        // This needs to happen early so we can determine the correct platform for mapping
        let auto_detected_compilers = pyproject_metadata_provider
            .build_system_requires()?
            .map(|reqs| detect_compilers_from_build_requirements(reqs))
            .unwrap_or_default();

        // Merge explicit config compilers with auto-detected ones
        let mut compilers = config.compilers.clone().unwrap_or_default();
        for compiler in auto_detected_compilers {
            if !compilers.contains(&compiler) {
                compilers.push(compiler);
            }
        }

        // Determine whether the package should be built as a noarch package.
        // This needs to be determined early so we can use the correct platform for PyPI mapping.
        let has_compilers = !compilers.is_empty();
        let is_noarch = if config.noarch == Some(true) {
            // The user explicitly requested a noarch package.
            true
        } else if config.noarch == Some(false) {
            // The user explicitly requested a non-noarch package.
            false
        } else if has_compilers {
            // No specific user request, but we have compilers, not a noarch package.
            false
        } else {
            // Otherwise, default to a noarch package.
            // This is the default behavior for pure Python packages.
            true
        };

        // Validate abi3 + noarch conflict
        if config.abi3 == Some(true) && is_noarch {
            miette::bail!(
                "abi3 = true is incompatible with noarch packages. \
                 The stable ABI is only meaningful for packages with compiled extensions."
            );
        }

        // Add python_abi host dependency when abi3 is enabled
        if config.abi3 == Some(true) {
            let requires_python_str = pyproject_metadata_provider.requires_python().ok().flatten();
            let abi_spec = python_abi_spec_from_requires_python(requires_python_str.as_deref())?;
            let python_abi_req: Item<PackageDependency> =
                format!("python_abi {abi_spec}").parse().into_diagnostic()?;
            requirements.host.push(python_abi_req);
        }

        // Use NoArch platform for mapping if this is a noarch package
        let mapping_platform = if is_noarch {
            Platform::NoArch
        } else {
            host_platform
        };

        // Map PyPI dependencies from pyproject.toml to conda dependencies
        if !config.ignore_pypi_mapping() {
            if let Some(pypi_deps) = pyproject_metadata_provider.project_dependencies()? {
                let mapped_deps = map_requirements_with_channels(
                    pypi_deps,
                    &channels,
                    &cache_dir,
                    "project",
                    mapping_platform,
                )
                .await;

                let skip_packages: HashSet<pixi_build_types::SourcePackageName> =
                    model_dependencies
                        .run
                        .keys()
                        .map(|k| (*k).clone())
                        .collect();

                for match_spec in filter_mapped_pypi_deps(&mapped_deps, &skip_packages) {
                    requirements
                        .run
                        .push(match_spec.to_string().parse().into_diagnostic()?);
                }
            }

            // Map build-system.requires from pyproject.toml to conda host dependencies
            if let Some(build_system_deps) = pyproject_metadata_provider.build_system_requires()? {
                let mapped_deps = map_requirements_with_channels(
                    build_system_deps,
                    &channels,
                    &cache_dir,
                    "build-system",
                    mapping_platform,
                )
                .await;

                let skip_packages: HashSet<pixi_build_types::SourcePackageName> =
                    model_dependencies
                        .host
                        .keys()
                        .map(|k| (*k).clone())
                        .collect();

                for match_spec in filter_mapped_pypi_deps(&mapped_deps, &skip_packages) {
                    requirements
                        .host
                        .push(match_spec.to_string().parse().into_diagnostic()?);
                }
            }
        }

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

        let build_platform = Platform::current();

        // TODO: remove this env var override as soon as we have profiles
        let editable = std::env::var("BUILD_EDITABLE_PYTHON")
            .map(|val| val == "true")
            .unwrap_or(params.editable);

        let build_script = BuildScriptContext {
            installer,
            build_platform: if build_platform.is_windows() {
                BuildPlatform::Windows
            } else {
                BuildPlatform::Unix
            },
            editable,
            extra_args: config.extra_args.clone(),
            manifest_root: manifest_root.clone(),
        }
        .render();

        // Convert the is_noarch boolean to the NoArchKind enum
        let noarch_kind = if is_noarch {
            Some(NoArchKind::Python)
        } else {
            None
        };

        // read pyproject.toml content if it exists
        let pyproject_manifest_path = manifest_root.join("pyproject.toml");
        let pyproject_manifest = if pyproject_manifest_path.exists() {
            let contents = fs::read_to_string(&pyproject_manifest_path).into_diagnostic()?;
            generated_recipe.build_input_globs =
                BTreeSet::from([pyproject_manifest_path.to_string_lossy().to_string()]);
            Some(toml::from_str(&contents).into_diagnostic()?)
        } else {
            None
        };

        // Construct python specific settings
        let python = Python {
            entry_points: PythonGenerator::entry_points(pyproject_manifest),
            version_independent: config.abi3 == Some(true),
        };

        generated_recipe.recipe.build.python = python;
        generated_recipe.recipe.build.noarch = noarch_kind;

        generated_recipe.recipe.build.script = Script {
            content: build_script,
            env: config.env.clone(),
            ..Script::default()
        };

        // Add the metadata input globs from the MetadataProvider
        generated_recipe
            .metadata_input_globs
            .extend(pyproject_metadata_provider.input_globs());

        // Log any warnings collected during metadata extraction
        for warning in pyproject_metadata_provider.warnings() {
            tracing::warn!("{}", warning);
        }

        Ok(generated_recipe)
    }

    /// Determines the build input globs for given python package
    /// even this will be probably backend specific, e.g setuptools
    /// has a different way of determining the input globs than hatch etc.
    ///
    /// However, lets take everything in the directory as input for now
    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        editable: bool,
    ) -> miette::Result<BTreeSet<String>> {
        let base_globs = Vec::from([
            // Project configuration
            "setup.py",
            "setup.cfg",
            "pyproject.toml",
            "requirements*.txt",
            "Pipfile",
            "Pipfile.lock",
            "poetry.lock",
            "tox.ini",
        ]);
        let compiler_based_globs: Vec<&str> = config
            .compilers
            .iter()
            .flatten()
            .flat_map(|c| match c.as_str() {
                "rust" => vec!["**/*.rs", "**/Cargo.toml"],
                "cxx" => vec!["**/*.{cc,cxx,cpp,hpp,hxx}"],
                "c" => vec!["**/*.{c,h}"],
                _ => vec![],
            })
            .collect();

        let python_globs = if editable {
            Vec::new()
        } else {
            Vec::from(["**/*.py", "**/*.pyx"])
        };

        Ok(base_globs
            .iter()
            .chain(python_globs.iter())
            .chain(compiler_based_globs.iter())
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
        IntermediateBackendInstantiator::<PythonGenerator>::new(log, Arc::default())
    })
    .await
    {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use pixi_build_backend::utils::test::intermediate_conda_outputs;
    use pixi_build_types::VariantValue;
    use recipe_stage0::recipe::{Item, Value};
    use tokio::fs;

    use super::*;

    #[test]
    fn test_input_globs_includes_extra_globs() {
        let config = PythonBackendConfig {
            extra_input_globs: vec!["custom/*.py".to_string()],
            ..Default::default()
        };

        let generator = PythonGenerator::default();

        let result = generator.extract_input_globs_from_build(&config, PathBuf::new(), false);

        insta::assert_debug_snapshot!(result);
    }

    #[test]
    fn test_input_globs_includes_extra_globs_editable() {
        let config = PythonBackendConfig {
            extra_input_globs: vec!["custom/*.py".to_string()],
            ..Default::default()
        };

        let generator = PythonGenerator::default();
        let result = generator.extract_input_globs_from_build(&config, PathBuf::new(), true);

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

        fs::write(
            temp_dir.path().join("pyproject.toml"),
            r#"[project]
name = "foobar"
version = "0.1.0"
"#,
        )
        .await
        .expect("Failed to write pyproject.toml");
        fs::write(
            temp_dir.path().join("pixi.toml"),
            r#"[project]
name = "foobar"
version = "0.1.0"
"#,
        )
        .await
        .expect("Failed to write pixi.toml");

        let variant_configuration = BTreeMap::from([(
            "boltons".to_string(),
            Vec::from([VariantValue::from("==1.0.0")]),
        )]);

        let result = intermediate_conda_outputs::<PythonGenerator>(
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
            assert_eq!(tp, &VariantValue::from("noarch"));
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

        fs::write(
            temp_dir.path().join("pyproject.toml"),
            r#"[project]
name = "foobar"
version = "0.1.0"
"#,
        )
        .await
        .expect("Failed to write pyproject.toml");
        fs::write(
            temp_dir.path().join("pixi.toml"),
            r#"[project]
name = "foobar"
version = "0.1.0"
"#,
        )
        .await
        .expect("Failed to write pixi.toml");

        let variant_file = temp_dir.path().join("variants.yaml");
        fs::write(
            &variant_file,
            r#"boltons:
  - "==2.0.0"
"#,
        )
        .await
        .expect("Failed to write variants file");

        let result = intermediate_conda_outputs::<PythonGenerator>(
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
            assert_eq!(tp, &VariantValue::from("noarch"));
        }
    }

    #[tokio::test]
    async fn test_pip_is_in_host_requirements() {
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

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                &PythonBackendConfig::default_with_ignore_pyproject_manifest(),
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
    async fn test_python_is_not_added_if_already_present() {
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

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                &PythonBackendConfig::default_with_ignore_pyproject_manifest(),
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

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                &PythonBackendConfig {
                    env: env.clone(),
                    ignore_pyproject_manifest: Some(true),
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

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                &PythonBackendConfig {
                    compilers: Some(vec!["c".to_string(), "cxx".to_string(), "rust".to_string()]),
                    ignore_pyproject_manifest: Some(true),
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
            compiler_templates.contains(&"${{ compiler('rust') }}".to_string()),
            "Rust compiler should be in build requirements"
        );
    }

    #[tokio::test]
    async fn test_default_no_compilers_when_not_specified() {
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

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                &PythonBackendConfig {
                    compilers: None,
                    ignore_pyproject_manifest: Some(true),
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

        // Check that no compilers are added by default
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let compiler_templates: Vec<String> = build_reqs
            .iter()
            .filter_map(|item| match item {
                Item::Value(Value::Template(s)) if s.contains("compiler") => Some(s.clone()),
                _ => None,
            })
            .collect();

        // Should have no compilers by default for Python packages
        assert_eq!(
            compiler_templates.len(),
            0,
            "Should have no compilers by default for pure Python packages"
        );
    }

    // Helper function to create a minimal project fixture
    fn minimal_project() -> pixi_build_types::ProjectModel {
        project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {}
            }
        })
    }

    // Helper function to generate recipe with given config
    async fn generate_test_recipe(
        config: &PythonBackendConfig,
    ) -> Result<GeneratedRecipe, Box<dyn std::error::Error>> {
        Ok(PythonGenerator::default()
            .generate_recipe(
                &minimal_project(),
                config,
                PathBuf::from("."),
                Platform::Linux64,
                None,
                &std::collections::HashSet::<pixi_build_backend::variants::NormalizedKey>::new(),
                vec![],
                None,
            )
            .await?)
    }

    #[tokio::test]
    async fn test_noarch_defaults_to_true_when_no_compilers() {
        let recipe = generate_test_recipe(&PythonBackendConfig {
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        })
        .await
        .expect("Failed to generate recipe");

        assert!(
            matches!(recipe.recipe.build.noarch, Some(NoArchKind::Python)),
            "noarch should default to true when no compilers specified"
        );
    }

    #[tokio::test]
    async fn test_noarch_defaults_to_false_when_compilers_present() {
        let config = PythonBackendConfig {
            compilers: Some(vec!["c".to_string()]),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };

        let recipe = generate_test_recipe(&config)
            .await
            .expect("Failed to generate recipe");

        assert!(
            recipe.recipe.build.noarch.is_none(),
            "noarch should default to false when compilers are present"
        );
    }

    #[tokio::test]
    async fn test_noarch_explicit_true_overrides_compilers() {
        let config = PythonBackendConfig {
            noarch: Some(true),
            compilers: Some(vec!["c".to_string()]),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };

        let recipe = generate_test_recipe(&config)
            .await
            .expect("Failed to generate recipe");

        assert!(
            matches!(recipe.recipe.build.noarch, Some(NoArchKind::Python)),
            "explicit noarch=true should override compiler presence"
        );
    }

    #[tokio::test]
    async fn test_noarch_explicit_false_overrides_no_compilers() {
        let config = PythonBackendConfig {
            noarch: Some(false),
            compilers: None,
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };

        let recipe = generate_test_recipe(&config)
            .await
            .expect("Failed to generate recipe");

        assert!(
            recipe.recipe.build.noarch.is_none(),
            "explicit noarch=false should override absence of compilers"
        );
    }

    #[test]
    fn test_c_compilers_create_extra_input_globs() {
        let config = PythonBackendConfig {
            compilers: Some(vec!["c".to_string()]),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };
        let generator = PythonGenerator::default();
        let result = generator.extract_input_globs_from_build(&config, PathBuf::new(), false);
        insta::assert_debug_snapshot!(result);
    }

    #[test]
    fn test_cxx_compilers_create_extra_input_globs() {
        let config = PythonBackendConfig {
            compilers: Some(vec!["cxx".to_string()]),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };
        let generator = PythonGenerator::default();
        let result = generator.extract_input_globs_from_build(&config, PathBuf::new(), false);
        insta::assert_debug_snapshot!(result);
    }

    #[test]
    fn test_rust_compilers_create_extra_input_globs() {
        let config = PythonBackendConfig {
            compilers: Some(vec!["rust".to_string()]),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };
        let generator = PythonGenerator::default();
        let result = generator.extract_input_globs_from_build(&config, PathBuf::new(), false);
        insta::assert_debug_snapshot!(result);
    }

    #[tokio::test]
    async fn test_ignore_pypi_mapping_skips_dependency_mapping() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
        });

        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

        // Create a pyproject.toml with dependencies that would be mapped
        fs::write(
            temp_dir.path().join("pyproject.toml"),
            r#"[project]
name = "foobar"
version = "0.1.0"
dependencies = ["requests>=2.28", "flask"]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"
"#,
        )
        .await
        .expect("Failed to write pyproject.toml");

        // Test with ignore_pypi_mapping = true
        let config = PythonBackendConfig {
            ignore_pypi_mapping: Some(true),
            ..Default::default()
        };

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                &config,
                temp_dir.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![ChannelUrl::from(
                    url::Url::parse("https://prefix.dev/conda-forge").unwrap(),
                )],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // With ignore_pypi_mapping = true, the pypi dependencies should NOT be mapped
        // Run requirements should only contain python (auto-added)
        let run_deps: Vec<String> = generated_recipe
            .recipe
            .requirements
            .run
            .iter()
            .map(|item| item.to_string())
            .collect();

        assert_eq!(
            run_deps,
            vec!["python"],
            "run deps should only contain python when ignore_pypi_mapping=true"
        );

        // Host requirements should only contain pip (auto-added installer) and python
        let host_deps: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|item| item.to_string())
            .collect();

        assert_eq!(
            host_deps,
            vec!["pip", "python"],
            "host deps should only contain pip and python when ignore_pypi_mapping=true"
        );
    }

    #[test]
    fn test_python_abi_spec_from_requires_python() {
        // Basic lower bound
        assert_eq!(
            python_abi_spec_from_requires_python(Some(">=3.9")).unwrap(),
            ">=3.9,<3.10.0a0"
        );
        // With patch version
        assert_eq!(
            python_abi_spec_from_requires_python(Some(">=3.9.3")).unwrap(),
            ">=3.9.3,<3.10.0a0"
        );
        // Multiple specifiers - uses the >= bound
        assert_eq!(
            python_abi_spec_from_requires_python(Some(">=3.11,<4")).unwrap(),
            ">=3.11,<3.12.0a0"
        );
        // 3.8 lower bound
        assert_eq!(
            python_abi_spec_from_requires_python(Some(">=3.8")).unwrap(),
            ">=3.8,<3.9.0a0"
        );
        // None defaults to 3.8
        assert_eq!(
            python_abi_spec_from_requires_python(None).unwrap(),
            ">=3.8,<3.9.0a0"
        );
        // Extra segments are preserved in lower bound but upper bound still pins to major.minor
        assert_eq!(
            python_abi_spec_from_requires_python(Some(">=3.9.3.4")).unwrap(),
            ">=3.9.3.4,<3.10.0a0"
        );
    }

    #[tokio::test]
    async fn test_abi3_adds_python_abi_to_host() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {}
            }
        });

        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");

        fs::write(
            temp_dir.path().join("pyproject.toml"),
            r#"[project]
name = "foobar"
version = "0.1.0"
requires-python = ">=3.9"

[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"
"#,
        )
        .await
        .expect("Failed to write pyproject.toml");

        let config = PythonBackendConfig {
            abi3: Some(true),
            noarch: Some(false),
            compilers: Some(vec!["c".to_string()]),
            ..Default::default()
        };

        let generated_recipe = PythonGenerator::default()
            .generate_recipe(
                &project_model,
                &config,
                temp_dir.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        let host_deps: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|item| item.to_string())
            .collect();

        assert!(
            host_deps.iter().any(|d| d.contains("python_abi")),
            "host deps should contain python_abi when abi3=true, got: {host_deps:?}"
        );
        // Check the version spec
        let abi_dep = host_deps.iter().find(|d| d.contains("python_abi")).unwrap();
        assert!(
            abi_dep.contains(">=3.9") && abi_dep.contains("<3.10.0a0"),
            "python_abi should have >=3.9,<3.10.0a0 spec, got: {abi_dep}"
        );
        // Check version_independent is set
        assert!(
            generated_recipe.recipe.build.python.version_independent,
            "version_independent should be true when abi3=true"
        );
    }

    #[tokio::test]
    async fn test_abi3_with_noarch_errors() {
        let config = PythonBackendConfig {
            abi3: Some(true),
            noarch: Some(true),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };

        let result = generate_test_recipe(&config).await;
        assert!(result.is_err(), "abi3=true with noarch=true should error");
    }

    #[tokio::test]
    async fn test_abi3_without_requires_python_defaults() {
        let config = PythonBackendConfig {
            abi3: Some(true),
            noarch: Some(false),
            compilers: Some(vec!["c".to_string()]),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        };

        let generated_recipe = generate_test_recipe(&config)
            .await
            .expect("Failed to generate recipe");

        let host_deps: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|item| item.to_string())
            .collect();

        let abi_dep = host_deps.iter().find(|d| d.contains("python_abi"));
        assert!(
            abi_dep.is_some(),
            "host deps should contain python_abi, got: {host_deps:?}"
        );
        let abi_dep = abi_dep.unwrap();
        assert!(
            abi_dep.contains(">=3.8") && abi_dep.contains("<3.9.0a0"),
            "python_abi should default to >=3.8,<3.9.0a0, got: {abi_dep}"
        );
    }

    #[tokio::test]
    async fn test_ignore_pypi_mapping_default_is_true() {
        // Verify that the default value for ignore_pypi_mapping is true
        let config = PythonBackendConfig::default();
        assert!(
            config.ignore_pypi_mapping(),
            "ignore_pypi_mapping should default to true"
        );
    }
}
