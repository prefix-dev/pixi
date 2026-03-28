mod build_script;
mod config;
mod metadata;

use build_script::{BuildPlatform, BuildScriptContext};
use config::RBackendConfig;
use metadata::DescriptionMetadataProvider;
use miette::IntoDiagnostic;
use pixi_build_backend::{
    Variable,
    generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams},
    intermediate_backend::IntermediateBackendInstantiator,
    traits::ProjectModel,
    variants::NormalizedKey,
};
use pixi_build_types::SourcePackageName;
use rattler_build_recipe::stage0::{Item, Script, SerializableMatchSpec, Value};
use rattler_conda_types::PackageName;
use rattler_conda_types::{ChannelUrl, Platform};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Parse a string into an `Item<SerializableMatchSpec>` for use in requirements.
fn matchspec_item(
    spec: &str,
) -> Result<Item<SerializableMatchSpec>, rattler_conda_types::ParseMatchSpecError> {
    Ok(Item::Value(Value::new_concrete(spec.parse()?, None)))
}

#[derive(Default, Clone)]
pub struct RGenerator {}

impl RGenerator {
    /// Detect if package has native code requiring compilers
    fn detect_native_code(manifest_root: &Path) -> bool {
        let src_dir = manifest_root.join("src");
        src_dir.exists() && src_dir.is_dir()
    }

    /// Auto-detect required compilers based on package structure
    fn auto_detect_compilers(
        manifest_root: &Path,
        provider: &DescriptionMetadataProvider,
    ) -> miette::Result<Vec<String>> {
        let has_native = Self::detect_native_code(manifest_root);
        let has_linking = provider.has_linking_to().into_diagnostic()?;

        if !has_native && !has_linking {
            return Ok(Vec::new());
        }

        // Default to C, C++, and Fortran for packages with native code
        // This covers most R packages with compiled code
        Ok(vec![
            "c".to_string(),
            "cxx".to_string(),
            "fortran".to_string(),
        ])
    }
}

#[async_trait::async_trait]
impl GenerateRecipe for RGenerator {
    type Config = RBackendConfig;

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
        // Determine the manifest root
        let manifest_root = if manifest_path.is_file() {
            manifest_path
                .parent()
                .ok_or_else(|| {
                    miette::miette!("Manifest path {} has no parent", manifest_path.display())
                })?
                .to_path_buf()
        } else {
            manifest_path.clone()
        };

        let mut metadata_provider = DescriptionMetadataProvider::new(&manifest_root);

        let mut generated_recipe =
            GeneratedRecipe::from_model(model.clone(), &mut metadata_provider).into_diagnostic()?;

        let requirements = &mut generated_recipe.recipe.requirements;
        let model_dependencies = model.dependencies(Some(host_platform));

        // Auto-detect or use configured compilers
        let compilers = match &config.compilers {
            Some(c) => c.clone(),
            None => Self::auto_detect_compilers(&manifest_root, &metadata_provider)?,
        };

        // Add compilers to build requirements
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

        // Add R runtime to host requirements
        let r_pkg = SourcePackageName::from(PackageName::new_unchecked("r-base"));
        if !model_dependencies.host.contains_key(&r_pkg) {
            requirements
                .host
                .push(matchspec_item("r-base").into_diagnostic()?);
        }

        // Add R runtime to run requirements
        if !model_dependencies.run.contains_key(&r_pkg) {
            requirements
                .run
                .push(matchspec_item("r-base").into_diagnostic()?);
        }

        // Add R package dependencies from DESCRIPTION (Imports + Depends)
        let r_dependencies = metadata_provider.runtime_dependencies().into_diagnostic()?;

        for dep in r_dependencies {
            // Skip packages that are built into r-base (base + recommended packages)
            if metadata::is_builtin_package(&dep.name) {
                continue;
            }

            // Convert R package name to conda package name
            let conda_name = metadata::r_package_to_conda(&dep.name);

            // Build the dependency spec string
            let dep_spec = if let Some(version) = &dep.version {
                let conda_version = metadata::r_version_to_conda(version);
                format!("{} {}", conda_name, conda_version)
            } else {
                conda_name
            };

            // Add to host requirements (runtime dependencies)
            requirements
                .host
                .push(matchspec_item(&dep_spec).into_diagnostic()?);

            // Also add to run requirements
            requirements
                .run
                .push(matchspec_item(&dep_spec).into_diagnostic()?);
        }

        // Add LinkingTo dependencies (packages providing headers for C/C++ compilation)
        let linking_to_deps = metadata_provider.linking_to().into_diagnostic()?;

        for dep in linking_to_deps {
            // Skip packages that are built into r-base
            if metadata::is_builtin_package(&dep.name) {
                continue;
            }

            // Convert R package name to conda package name
            let conda_name = metadata::r_package_to_conda(&dep.name);

            // Build the dependency spec string
            let dep_spec = if let Some(version) = &dep.version {
                let conda_version = metadata::r_version_to_conda(version);
                format!("{} {}", conda_name, conda_version)
            } else {
                conda_name
            };

            // Add to host requirements only (LinkingTo packages provide headers at compile time)
            requirements
                .host
                .push(matchspec_item(&dep_spec).into_diagnostic()?);
        }

        // Generate build script
        let has_native_code = !compilers.is_empty();
        let build_script = BuildScriptContext {
            build_platform: if Platform::current().is_windows() {
                BuildPlatform::Windows
            } else {
                BuildPlatform::Unix
            },
            source_dir: manifest_root.display().to_string(),
            extra_args: config.extra_args.clone(),
            has_native_code,
        }
        .render();

        generated_recipe.recipe.build.script = Script::from_content(build_script).with_env(
            config
                .env
                .iter()
                .map(|(k, v)| (k.clone(), Value::new_concrete(v.clone(), None)))
                .collect(),
        );

        // Add metadata input globs
        generated_recipe
            .metadata_input_globs
            .extend(metadata_provider.input_globs());

        Ok(generated_recipe)
    }

    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<BTreeSet<String>> {
        let mut globs = BTreeSet::from(
            [
                // R package structure files
                "DESCRIPTION",
                "NAMESPACE",
                "**/*.R",  // R source files
                "**/*.Rd", // R documentation
            ]
            .map(String::from),
        );

        // Add compiler-specific globs if compilers are configured
        if let Some(compilers) = &config.compilers {
            for compiler in compilers {
                match compiler.as_str() {
                    "c" => {
                        globs.insert("**/*.c".to_string());
                        globs.insert("**/*.h".to_string());
                    }
                    "cxx" => {
                        globs.insert("**/*.cpp".to_string());
                        globs.insert("**/*.cc".to_string());
                        globs.insert("**/*.cxx".to_string());
                        globs.insert("**/*.hpp".to_string());
                        globs.insert("**/*.hxx".to_string());
                    }
                    "fortran" => {
                        globs.insert("**/*.f".to_string());
                        globs.insert("**/*.f90".to_string());
                        globs.insert("**/*.f95".to_string());
                    }
                    _ => {}
                }
            }
        }

        // Add extra globs from config
        globs.extend(config.extra_input_globs.clone());

        Ok(globs)
    }

    fn default_variants(
        &self,
        _host_platform: Platform,
    ) -> miette::Result<BTreeMap<NormalizedKey, Vec<Variable>>> {
        // R packages don't typically need special default variants
        // Compiler variants are handled by rattler-build defaults
        Ok(BTreeMap::new())
    }
}

#[tokio::main]
pub async fn main() {
    if let Err(err) = pixi_build_backend::cli::main(|log| {
        IntermediateBackendInstantiator::<RGenerator>::new(log, Arc::default())
    })
    .await
    {
        eprintln!("{err:?}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::fs;

    #[macro_export]
    macro_rules! project_fixture {
        ($($json:tt)+) => {
            serde_json::from_value::<pixi_build_types::ProjectModel>(
                serde_json::json!($($json)+)
            ).expect("Failed to create project model from JSON")
        };
    }

    #[tokio::test]
    async fn test_r_package_with_native_code() {
        let temp_dir = TempDir::new().unwrap();

        // Create DESCRIPTION file
        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            r#"Package: testpkg
Version: 1.0.0
Title: Test Package
Description: A test package
License: GPL-3
LinkingTo: Rcpp
"#,
        )
        .await
        .unwrap();

        // Create src directory to trigger native code detection
        fs::create_dir(temp_dir.path().join("src")).await.unwrap();

        let project_model = project_fixture!({
            "name": "r-testpkg",
            "version": "1.0.0",
            "targets": {
                "defaultTarget": {}
            }
        });

        let generated_recipe = RGenerator::default()
            .generate_recipe(
                &project_model,
                &RBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Verify compilers were added
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let has_compilers = build_reqs.iter().any(|item| match item {
            Item::Value(v) => v
                .as_template()
                .is_some_and(|t| t.to_string().contains("compiler")),
            _ => false,
        });
        assert!(has_compilers, "Native code package should have compilers");

        // Verify r-base is in host and run requirements
        let host_reqs = &generated_recipe.recipe.requirements.host;
        let has_r_base_host = host_reqs
            .iter()
            .any(|req| req.to_string().starts_with("r-base"));
        assert!(has_r_base_host, "Should have r-base in host requirements");

        let run_reqs = &generated_recipe.recipe.requirements.run;
        let has_r_base_run = run_reqs
            .iter()
            .any(|req| req.to_string().starts_with("r-base"));
        assert!(has_r_base_run, "Should have r-base in run requirements");

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
            ".source[0].path" => "[path]",
            ".build.script.content" => "[build_script]",
        });
    }

    #[tokio::test]
    async fn test_pure_r_package_no_compilers() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: purepkg\nVersion: 1.0.0\nTitle: Pure R Package\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-purepkg",
            "version": "1.0.0",
            "targets": {
                "defaultTarget": {}
            }
        });

        let generated_recipe = RGenerator::default()
            .generate_recipe(
                &project_model,
                &RBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Verify no compilers were added for pure R package
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let has_compilers = build_reqs.iter().any(|item| match item {
            Item::Value(v) => v
                .as_template()
                .is_some_and(|t| t.to_string().contains("compiler")),
            _ => false,
        });
        assert!(!has_compilers, "Pure R package should not have compilers");

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
            ".source[0].path" => "[path]",
            ".build.script.content" => "[build_script]",
        });
    }

    #[test]
    fn test_input_globs_for_r_package() {
        let config = RBackendConfig {
            compilers: Some(vec!["c".to_string(), "cxx".to_string()]),
            ..Default::default()
        };

        let generator = RGenerator::default();
        let globs = generator
            .extract_input_globs_from_build(&config, PathBuf::new(), false)
            .unwrap();

        assert!(globs.contains("DESCRIPTION"));
        assert!(globs.contains("NAMESPACE"));
        assert!(globs.contains("**/*.R"));
        assert!(globs.contains("**/*.c"));
        assert!(globs.contains("**/*.cpp"));
    }

    #[test]
    fn test_input_globs_with_extra_globs() {
        let config = RBackendConfig {
            extra_input_globs: vec!["inst/**/*".to_string()],
            ..Default::default()
        };

        let generator = RGenerator::default();
        let globs = generator
            .extract_input_globs_from_build(&config, PathBuf::new(), false)
            .unwrap();

        assert!(globs.contains("inst/**/*"));
    }

    #[tokio::test]
    async fn test_r_package_with_dependencies() {
        let temp_dir = TempDir::new().unwrap();

        // Create DESCRIPTION file with dependencies
        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            r#"Package: webmockr
Version: 2.2.1.92
Depends:
    R(>= 4.1.0)
Imports:
    curl,
    jsonlite,
    magrittr (>= 1.5),
    R6 (>= 2.1.3),
    urltools (>= 1.6.0)
"#,
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-webmockr",
            "version": "2.2.1.92",
            "targets": {
                "defaultTarget": {}
            }
        });

        let generated_recipe = RGenerator::default()
            .generate_recipe(
                &project_model,
                &RBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        // Verify r-base is in host and run requirements
        let host_reqs = &generated_recipe.recipe.requirements.host;
        let run_reqs = &generated_recipe.recipe.requirements.run;

        // Check for r-base
        assert!(
            host_reqs
                .iter()
                .any(|req| req.to_string().starts_with("r-base")),
            "Should have r-base in host requirements"
        );

        // Check for r-curl
        assert!(
            host_reqs
                .iter()
                .any(|req| req.to_string().starts_with("r-curl")),
            "Should have r-curl in host requirements"
        );

        // Check for r-jsonlite
        assert!(
            host_reqs
                .iter()
                .any(|req| req.to_string().starts_with("r-jsonlite")),
            "Should have r-jsonlite in host requirements"
        );

        // Check for r-magrittr with version constraint
        let has_magrittr = host_reqs.iter().any(|req| {
            let s = req.to_string();
            s.contains("r-magrittr") && s.contains(">=1.5")
        });
        assert!(
            has_magrittr,
            "Should have r-magrittr >=1.5 in host requirements"
        );

        // Check for r-r6 (R6 -> r-r6)
        let has_r6 = host_reqs.iter().any(|req| {
            let s = req.to_string();
            s.contains("r-r6") && s.contains(">=2.1.3")
        });
        assert!(has_r6, "Should have r-r6 >=2.1.3 in host requirements");

        // Verify same dependencies are in run requirements
        assert!(
            run_reqs
                .iter()
                .any(|req| req.to_string().starts_with("r-curl")),
            "Should have r-curl in run requirements"
        );
        assert!(
            run_reqs
                .iter()
                .any(|req| req.to_string().starts_with("r-jsonlite")),
            "Should have r-jsonlite in run requirements"
        );

        insta::assert_yaml_snapshot!(generated_recipe.recipe, {
            ".source[0].path" => "[path]",
            ".build.script.content" => "[build_script]",
        });
    }

    #[tokio::test]
    async fn test_explicit_compilers_override() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: testpkg\nVersion: 1.0.0\n",
        )
        .await
        .unwrap();

        // Create src directory (would normally trigger auto-detection)
        fs::create_dir(temp_dir.path().join("src")).await.unwrap();

        let project_model = project_fixture!({
            "name": "r-testpkg",
            "version": "1.0.0",
            "targets": {
                "defaultTarget": {}
            }
        });

        // Explicitly specify only C compiler
        let config = RBackendConfig {
            compilers: Some(vec!["c".to_string()]),
            ..Default::default()
        };

        let generated_recipe = RGenerator::default()
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

        // Verify only one compiler template was added
        let build_reqs = &generated_recipe.recipe.requirements.build;
        let compiler_count = build_reqs
            .iter()
            .filter(|item| match item {
                Item::Value(v) => v
                    .as_template()
                    .is_some_and(|t| t.to_string().contains("compiler('c')")),
                _ => false,
            })
            .count();

        assert_eq!(
            compiler_count, 1,
            "Should have exactly one compiler when explicitly set"
        );
    }
}
