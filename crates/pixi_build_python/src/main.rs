mod build_script;
mod config;
mod metadata;
mod pypi_mapping;

use build_script::{BuildPlatform, BuildScriptContext};
use config::PythonBackendConfig;
use fs_err as fs;
use miette::IntoDiagnostic;
use pixi_build_backend::variants::NormalizedKey;
use pixi_build_backend::{
    Variable,
    compilers::default_compiler_variants,
    generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    tools::BackendIdentifier,
};
use pyproject_toml::PyProjectToml;
use rattler_build_recipe::stage0::{
    ConditionalList, Item, PythonBuild, Script, SerializableMatchSpec, Value,
};
use rattler_conda_types::{
    ChannelUrl, NoArchType, PackageName, Platform, Version, package::EntryPoint,
};
use std::collections::HashSet;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use crate::metadata::{PyprojectManifestMode, PyprojectMetadataProvider};
use crate::pypi_mapping::{
    detect_compilers_from_build_requirements, map_requirements_with_channels,
};

const CYTHON_INPUT_GLOBS: &[&str] = &["**/*.{pyx,pxd,pxi}"];

/// Compute the `python-abi3` version spec from an optional `requires-python`
/// specifier string.
///
/// Extracts the lower bound (first `>=` specifier) and pins it to a single
/// minor version:
/// - `">=3.9"`     → `"3.9.*"`
/// - `">=3.9.3"`   → `"3.9.*"`
/// - `">=3.11,<4"` → `"3.11.*"`
/// - `None`        → `"3.9.*"` (default)
fn python_abi3_spec_from_requires_python(requires_python: Option<&str>) -> miette::Result<String> {
    let lower_bound = requires_python
        .and_then(|s| {
            let specifiers = pep440_rs::VersionSpecifiers::from_str(s).ok()?;
            specifiers
                .iter()
                .find(|spec| *spec.operator() == pep440_rs::Operator::GreaterThanEqual)
                .map(|spec| {
                    let pep_version = spec.version();
                    Version::from_str(&pep_version.to_string())
                        .expect("pep440 version should be a valid conda version")
                })
        })
        .unwrap_or_else(|| Version::from_str("3.9").expect("valid version"));

    let segment_count = std::cmp::min(lower_bound.segment_count(), 2);
    let major_minor = lower_bound
        .with_segments(..segment_count)
        .ok_or_else(|| miette::miette!("failed to truncate version to major.minor"))?;

    Ok(format!("{major_minor}.*"))
}

fn requirement_contains_package(
    requirements: &ConditionalList<SerializableMatchSpec>,
    package_name: &str,
) -> bool {
    requirements
        .iter()
        .any(|item| requirement_item_contains_package(item, package_name))
}

fn requirement_item_contains_package(
    item: &Item<SerializableMatchSpec>,
    package_name: &str,
) -> bool {
    match item {
        Item::Value(value) => {
            value
                .as_concrete()
                .and_then(|spec| spec.0.name.as_exact())
                .is_some_and(|name| name.as_normalized() == package_name)
                || value.to_string().split_whitespace().next() == Some(package_name)
        }
        Item::Conditional(cond) => cond
            .then
            .iter()
            .chain(cond.else_value.iter().flat_map(|items| items.iter()))
            .any(|item| requirement_item_contains_package(item, package_name)),
    }
}

fn package_name_item_contains_package(item: &Item<PackageName>, package_name: &str) -> bool {
    match item {
        Item::Value(value) => {
            value
                .as_concrete()
                .is_some_and(|name| name.as_normalized() == package_name)
                || value
                    .as_template()
                    .is_some_and(|template| template.as_str() == package_name)
        }
        Item::Conditional(cond) => cond
            .then
            .iter()
            .chain(cond.else_value.iter().flat_map(|items| items.iter()))
            .any(|item| package_name_item_contains_package(item, package_name)),
    }
}

/// Parse a string into an `Item<SerializableMatchSpec>` for use in requirements.
fn matchspec_item(
    spec: &str,
) -> Result<Item<SerializableMatchSpec>, rattler_conda_types::ParseMatchSpecError> {
    Ok(Item::Value(Value::new_concrete(spec.parse()?, None)))
}

#[derive(Default, Clone)]
pub struct PythonGenerator {}

impl PythonGenerator {
    /// Read the entry points from the pyproject.toml and return them as a list.
    ///
    /// If the manifest is not a pyproject.toml file no entry-points are added.
    pub(crate) fn entry_points(
        pyproject_manifest: Option<PyProjectToml>,
    ) -> ConditionalList<EntryPoint> {
        let scripts = pyproject_manifest
            .as_ref()
            .and_then(|p| p.project.as_ref())
            .and_then(|p| p.scripts.as_ref());

        let items: Vec<Item<EntryPoint>> = scripts
            .into_iter()
            .flatten()
            .flat_map(|(name, entry_point)| {
                EntryPoint::from_str(&format!("{name} = {entry_point}"))
                    .map(|ep| Item::Value(Value::new_concrete(ep, None)))
            })
            .collect();

        ConditionalList::new(items)
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
        _workspace_scratch_directory: Option<PathBuf>,
        _workspace_directory: Option<PathBuf>,
        _checkout_root: Option<PathBuf>,
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

        // Ensure the python build tools are added to the `host` requirements.
        // Please note: this is a subtle difference for python, where the build tools
        // are added to the `host` requirements, while for cmake/rust they are
        // added to the `build` requirements.
        let installer = config.installer.clone().unwrap_or_default();
        let installer_pkg = installer.package_name();
        requirements
            .host
            .push(matchspec_item(installer_pkg.as_ref()).into_diagnostic()?);

        // Get Python requirement spec
        let python_requirement_str = match pyproject_metadata_provider.requires_python() {
            Ok(Some(requires_python)) => format!("python {requires_python}"),
            _ => "python".to_string(),
        };

        // Add python to host and run requirements. A user-provided python spec
        // intersects with this one in the solver, so duplicates are harmless.
        let python_requirement = matchspec_item(&python_requirement_str).into_diagnostic()?;
        requirements.host.push(python_requirement.clone());
        requirements.run.push(python_requirement);

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

        // ABI3 packages should not inherit CPython ABI pins from `host: python`.
        if config.abi3 == Some(true) {
            if !requirement_contains_package(&requirements.host, "python-abi3") {
                let requires_python_str =
                    pyproject_metadata_provider.requires_python().ok().flatten();
                let abi3_spec =
                    python_abi3_spec_from_requires_python(requires_python_str.as_deref())?;
                let python_abi3_req =
                    matchspec_item(&format!("python-abi3 {abi3_spec}")).into_diagnostic()?;
                requirements.host.push(python_abi3_req);
            }

            let python_package = PackageName::from_str("python").into_diagnostic()?;
            if !requirements
                .ignore_run_exports
                .from_package
                .iter()
                .any(|item| package_name_item_contains_package(item, "python"))
            {
                requirements
                    .ignore_run_exports
                    .from_package
                    .push(Item::Value(Value::new_concrete(python_package, None)));
            }
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

                for dep in &mapped_deps {
                    requirements
                        .run
                        .push(matchspec_item(&dep.to_match_spec().to_string()).into_diagnostic()?);
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

                for dep in &mapped_deps {
                    requirements
                        .host
                        .push(matchspec_item(&dep.to_match_spec().to_string()).into_diagnostic()?);
                }
            }
        }

        pixi_build_backend::compilers::add_compilers_to_requirements(
            &compilers,
            &mut requirements.build,
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

        // Convert the is_noarch boolean to the NoArchType value
        let noarch_kind = if is_noarch {
            Some(Value::new_concrete(NoArchType::python(), None))
        } else {
            None
        };

        // read pyproject.toml content if it exists
        let pyproject_manifest_path = manifest_root.join("pyproject.toml");
        let pyproject_manifest = if pyproject_manifest_path.exists() {
            let contents = fs::read_to_string(&pyproject_manifest_path).into_diagnostic()?;
            generated_recipe.build_input_globs =
                vec![pyproject_manifest_path.to_string_lossy().to_string()];
            Some(toml::from_str(&contents).into_diagnostic()?)
        } else {
            None
        };

        // Construct python specific settings
        let skip_pyc_globs = config.skip_pyc_compilation.globs();
        let skip_pyc_compilation = ConditionalList::new(
            skip_pyc_globs
                .into_iter()
                .map(|g| Item::Value(Value::new_concrete(g, None)))
                .collect(),
        );
        let python = PythonBuild {
            entry_points: PythonGenerator::entry_points(pyproject_manifest),
            version_independent: if config.abi3 == Some(true) {
                Some(Value::new_concrete(true, None))
            } else {
                None
            },
            skip_pyc_compilation,
            ..PythonBuild::default()
        };

        generated_recipe.recipe.build.python = python;
        generated_recipe.recipe.build.noarch = noarch_kind;

        generated_recipe.recipe.build.script = Script::from_content(build_script)
            .with_env(
                config
                    .env
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::new_concrete(v.clone(), None)))
                    .collect(),
            )
            .with_secrets(model.secrets.iter().cloned().collect());

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
    ) -> miette::Result<Vec<String>> {
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
            Vec::from(["**/*.py"])
        };

        Ok(base_globs
            .iter()
            .chain(python_globs.iter())
            .chain(compiler_based_globs.iter())
            .map(|s| s.to_string())
            .chain(config.extra_input_globs.clone())
            .collect())
    }

    /// Cython inputs affect compiled extension artifacts even when the
    /// Python package itself is installed editable. The resolved packages
    /// also cover conditional dependencies, which are invisible in the
    /// manifest's default target.
    fn extract_input_globs_from_resolved_packages(
        &self,
        _config: &Self::Config,
        resolved_packages: &HashSet<rattler_conda_types::PackageName>,
    ) -> Vec<String> {
        let cython = rattler_conda_types::PackageName::new_unchecked("cython");
        if resolved_packages.contains(&cython) {
            CYTHON_INPUT_GLOBS
                .iter()
                .map(|glob| (*glob).to_string())
                .collect()
        } else {
            Vec::new()
        }
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
        IntermediateBackendInstantiator::<PythonGenerator>::new(
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
    use std::collections::BTreeMap;

    use indexmap::IndexMap;
    use pixi_build_backend::utils::test::intermediate_conda_outputs;
    use pixi_build_types::VariantValue;
    use rattler_build_recipe::stage0::Item;
    use tokio::fs;

    use super::*;
    use crate::build_script::Installer;

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
    async fn test_uv_is_in_host_requirements() {
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
    async fn test_pip_installer_from_config() {
        let generated_recipe = generate_test_recipe(&PythonBackendConfig {
            installer: Some(Installer::Pip),
            ignore_pyproject_manifest: Some(true),
            ..Default::default()
        })
        .await
        .expect("Failed to generate recipe");

        insta::assert_yaml_snapshot!(generated_recipe.recipe.requirements, @r###"
        host:
          - pip
          - python
        run:
          - python
        "###);
    }

    #[tokio::test]
    async fn test_python_is_added_even_if_already_present() {
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
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // The user spec and the backend-derived spec both land in the recipe
        // and intersect in the solver.
        insta::assert_yaml_snapshot!(generated_recipe.recipe.requirements, @r###"
        host:
          - python
          - uv
          - python
        run:
          - boltons
          - python
        "###);
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
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Check that no compilers are added by default
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
                None,
                None,
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
            recipe
                .recipe
                .build
                .noarch
                .as_ref()
                .and_then(|v| v.as_concrete())
                .map(|t| t.is_python())
                == Some(true),
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
            recipe
                .recipe
                .build
                .noarch
                .as_ref()
                .and_then(|v| v.as_concrete())
                .map(|t| t.is_python())
                == Some(true),
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
    fn test_cython_input_globs_added_when_cython_is_resolved() {
        let resolved_packages = HashSet::from([
            rattler_conda_types::PackageName::new_unchecked("python"),
            rattler_conda_types::PackageName::new_unchecked("cython"),
        ]);

        let globs = PythonGenerator::default().extract_input_globs_from_resolved_packages(
            &PythonBackendConfig::default(),
            &resolved_packages,
        );

        assert!(globs.iter().any(|g| g == CYTHON_INPUT_GLOBS[0]));
    }

    #[test]
    fn test_cython_input_globs_not_added_without_resolved_cython() {
        let resolved_packages =
            HashSet::from([rattler_conda_types::PackageName::new_unchecked("python")]);

        let globs = PythonGenerator::default().extract_input_globs_from_resolved_packages(
            &PythonBackendConfig::default(),
            &resolved_packages,
        );

        assert!(globs.is_empty());
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
                None,
                None,
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

        // Host requirements should only contain uv (auto-added default installer) and python
        let host_deps: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|item| item.to_string())
            .collect();

        assert_eq!(
            host_deps,
            vec!["uv", "python"],
            "host deps should only contain uv and python when ignore_pypi_mapping=true"
        );
    }

    #[tokio::test]
    async fn test_abi3_marks_recipe_version_independent_and_ignores_python_run_exports() {
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
                None,
                None,
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
            host_deps.iter().any(|d| d == "python-abi3 3.9.*"),
            "host deps should contain python-abi3 3.9.* when abi3=true, got: {host_deps:?}"
        );
        assert!(
            !host_deps.iter().any(|d| d.contains("python_abi")),
            "host deps should not contain python_abi when abi3=true, got: {host_deps:?}"
        );
        assert!(
            generated_recipe
                .recipe
                .build
                .python
                .version_independent
                .as_ref()
                .and_then(|v| v.as_concrete())
                .copied()
                == Some(true),
            "version_independent should be true when abi3=true"
        );

        let ignored_packages = &generated_recipe
            .recipe
            .requirements
            .ignore_run_exports
            .from_package;
        assert!(
            ignored_packages
                .iter()
                .any(|item| package_name_item_contains_package(item, "python")),
            "ignore_run_exports.from_package should contain python when abi3=true, got: {ignored_packages:?}"
        );

        let recipe_json = serde_json::to_string(&generated_recipe.recipe).unwrap();
        assert!(
            recipe_json.contains("ignore_run_exports"),
            "serialized recipe should include ignore_run_exports when abi3=true, got:\n{recipe_json}"
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

        assert!(
            host_deps.iter().any(|d| d == "python-abi3 3.9.*"),
            "host deps should contain python-abi3 3.9.* when abi3=true, got: {host_deps:?}"
        );
        assert!(
            !host_deps.iter().any(|d| d.contains("python_abi")),
            "host deps should not contain python_abi when abi3=true, got: {host_deps:?}"
        );
        assert!(
            generated_recipe
                .recipe
                .requirements
                .ignore_run_exports
                .from_package
                .iter()
                .any(|item| package_name_item_contains_package(item, "python")),
            "ignore_run_exports.from_package should contain python when abi3=true"
        );
    }

    #[tokio::test]
    async fn test_abi3_does_not_duplicate_explicit_python_abi3_dependency() {
        let project_model = project_fixture!({
            "name": "foobar",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {
                        "python-abi3": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                }
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
                None,
                None,
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

        assert_eq!(
            host_deps
                .iter()
                .filter(|dep| dep.starts_with("python-abi3"))
                .count(),
            1,
            "host deps should contain exactly one python-abi3 entry when it is explicitly declared, got: {host_deps:?}"
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
