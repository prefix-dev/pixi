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
    tools::BackendIdentifier,
    variants::NormalizedKey,
};
use rattler_build_recipe::stage0::{ConditionalList, Item, Script, SerializableMatchSpec, Value};
use rattler_conda_types::{ChannelUrl, Subdir};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Parse a string into an `Item<SerializableMatchSpec>` for use in requirements.
fn matchspec_item(
    spec: &str,
) -> Result<Item<SerializableMatchSpec>, rattler_conda_types::ParseMatchSpecError> {
    Ok(Item::Value(Value::new_concrete(spec.parse()?, None)))
}

fn requirement_contains_package(
    requirements: &ConditionalList<SerializableMatchSpec>,
    package_names: &[&str],
) -> bool {
    requirements
        .iter()
        .any(|item| requirement_item_contains_any_package(item, package_names))
}

/// Whether a requirement is for one of `package_names`, by its exact conda name.
fn requirement_item_contains_any_package(
    item: &Item<SerializableMatchSpec>,
    package_names: &[&str],
) -> bool {
    match item {
        Item::Value(value) => value
            .as_concrete()
            .and_then(|spec| spec.0.name.as_exact())
            .is_some_and(|name| {
                package_names
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(name.as_normalized()))
            }),
        Item::Conditional(cond) => cond
            .then
            .iter()
            .chain(cond.else_value.iter().flat_map(|items| items.iter()))
            .any(|item| requirement_item_contains_any_package(item, package_names)),
    }
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
        host_platform: Subdir,
        _python_params: Option<PythonParams>,
        variants: &HashSet<NormalizedKey>,
        channels: Vec<ChannelUrl>,
        _cache_dir: Option<PathBuf>,
        _workspace_scratch_directory: Option<PathBuf>,
        _workspace_directory: Option<PathBuf>,
        _checkout_root: Option<PathBuf>,
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

        // Auto-detect or use configured compilers
        let compilers = match &config.compilers {
            Some(c) => c.clone(),
            None => Self::auto_detect_compilers(&manifest_root, &metadata_provider)?,
        };

        // Add compilers to build requirements
        pixi_build_backend::compilers::add_compilers_to_requirements(
            &compilers,
            &mut requirements.build,
        );
        pixi_build_backend::compilers::add_stdlib_to_requirements(
            &compilers,
            &mut requirements.build,
            variants,
        );

        // Rust, for packages with Rust code. The plain `rust` package, as
        // conda-forge's R recipes use (e.g. r-gifski), rather than
        // `compiler('rust')`, with which R's Makevars didn't find the library
        // cargo built.
        if metadata_provider.needs_rust().into_diagnostic()? {
            requirements
                .build
                .push(matchspec_item("rust").into_diagnostic()?);
            // On Windows, R packages build their Rust code for the GNU target,
            // whose standard library `rust` doesn't bring
            if host_platform == Subdir::Win64 {
                requirements
                    .build
                    .push(matchspec_item("rust-std-x86_64-pc-windows-gnu").into_diagnostic()?);
            }
        }

        // CMake and pkg-config, when `SystemRequirements` names them.
        // Otherwise the build uses whichever is on the machine's PATH, or
        // fails without one.
        if metadata_provider.needs_cmake().into_diagnostic()? {
            requirements
                .build
                .push(matchspec_item("cmake").into_diagnostic()?);
        }
        if metadata_provider.needs_pkg_config().into_diagnostic()? {
            requirements
                .build
                .push(matchspec_item("pkg-config").into_diagnostic()?);
        }

        // Add the R runtime to host and run requirements. A user-provided
        // r-base spec intersects with this one in the solver, so duplicates
        // are harmless.
        requirements
            .host
            .push(matchspec_item("r-base").into_diagnostic()?);
        requirements
            .run
            .push(matchspec_item("r-base").into_diagnostic()?);

        // Add R package dependencies from DESCRIPTION (Imports + Depends) unless ignored
        if !config.ignore_description_dependencies.unwrap_or(false) {
            let r_dependencies = metadata_provider.runtime_dependencies().into_diagnostic()?;
            // Bioconductor packages among the dependencies, which come from bioconda
            let mut from_bioconda = Vec::new();

            for dep in r_dependencies {
                // Skip packages that are built into r-base (the base packages)
                if metadata::is_builtin_package(&dep.name) {
                    continue;
                }

                // Convert R package name to conda package name
                let conda_name = metadata::r_package_to_conda(&dep.name);
                if conda_name.starts_with("bioconductor-") {
                    from_bioconda.push(dep.name.clone());
                }

                let lower_dep = dep.name.to_lowercase();
                let r_name = format!("r-{lower_dep}");
                let bioc_name = format!("bioconductor-{lower_dep}");
                // The R package's possible conda names, but not its bare R name,
                // which can be another conda package: the R package openssl is
                // r-openssl, while openssl is the C library.
                let candidate_names = [r_name.as_str(), bioc_name.as_str()];

                // Build the dependency spec string
                let dep_spec = if let Some(version) = &dep.version {
                    let conda_version = metadata::r_version_to_conda(version);
                    format!("{} {}", conda_name, conda_version)
                } else {
                    conda_name.clone()
                };

                // Add to host requirements (runtime dependencies) if not already present
                if !requirement_contains_package(&requirements.host, &candidate_names) {
                    requirements
                        .host
                        .push(matchspec_item(&dep_spec).into_diagnostic()?);
                }

                // Also add to run requirements if not already present
                if !requirement_contains_package(&requirements.run, &candidate_names) {
                    requirements
                        .run
                        .push(matchspec_item(&dep_spec).into_diagnostic()?);
                }
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
                if conda_name.starts_with("bioconductor-") {
                    from_bioconda.push(dep.name.clone());
                }

                let lower_dep = dep.name.to_lowercase();
                let r_name = format!("r-{lower_dep}");
                let bioc_name = format!("bioconductor-{lower_dep}");
                // The R package's possible conda names, but not its bare R name,
                // which can be another conda package: the R package openssl is
                // r-openssl, while openssl is the C library.
                let candidate_names = [r_name.as_str(), bioc_name.as_str()];

                if !requirement_contains_package(&requirements.host, &candidate_names) {
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
            }

            // Without bioconda, the solver would only say it found no
            // `bioconductor-*` candidates. The channels are empty when they
            // aren't known, e.g. in tests.
            let has_bioconda = channels
                .iter()
                .any(|channel| channel.as_str().contains("bioconda"));
            if !from_bioconda.is_empty() && !channels.is_empty() && !has_bioconda {
                from_bioconda.sort();
                from_bioconda.dedup();
                miette::bail!(
                    help = "Add bioconda to the workspace's channels, e.g. with `pixi workspace channel add bioconda`.",
                    "This package depends on Bioconductor packages ({}), which come from bioconda as `bioconductor-<name>`, but bioconda isn't among the channels.",
                    from_bioconda.join(", ")
                );
            }
        }

        // Generate build script
        let has_native_code = !compilers.is_empty();
        let build_script = BuildScriptContext {
            build_platform: if Subdir::current().unwrap_or(Subdir::NoArch).is_windows() {
                BuildPlatform::Windows
            } else {
                BuildPlatform::Unix
            },
            source_dir: manifest_root.display().to_string(),
            extra_args: config.extra_args.clone(),
            has_native_code,
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
    ) -> miette::Result<Vec<String>> {
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

        Ok(globs.into_iter().collect())
    }

    fn default_variants(
        &self,
        _host_platform: Subdir,
    ) -> miette::Result<BTreeMap<NormalizedKey, Vec<Variable>>> {
        // R packages don't typically need special default variants
        // Compiler variants are handled by rattler-build defaults
        Ok(BTreeMap::new())
    }
}

#[tokio::main]
pub async fn main() {
    if let Err(err) = pixi_build_backend::cli::main(|log| {
        IntermediateBackendInstantiator::<RGenerator>::new(
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
                Subdir::Linux64,
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
    async fn test_r_base_is_added_even_if_already_present() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: testpkg\nVersion: 1.0.0\nTitle: Test Package\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-testpkg",
            "version": "1.0.0",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {
                        "r-base": {
                            "binary": {
                                "version": ">=4"
                            }
                        }
                    },
                    "runDependencies": {
                        "r-base": {
                            "binary": {
                                "version": ">=4"
                            }
                        }
                    }
                }
            }
        });

        let generated_recipe = RGenerator::default()
            .generate_recipe(
                &project_model,
                &RBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Subdir::Linux64,
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

        // The user spec and the backend-added spec both land in the recipe
        // and intersect in the solver.
        let host_count = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .filter(|req| req.to_string().starts_with("r-base"))
            .count();
        assert_eq!(host_count, 2, "expected user and backend r-base in host");

        let run_count = generated_recipe
            .recipe
            .requirements
            .run
            .iter()
            .filter(|req| req.to_string().starts_with("r-base"))
            .count();
        assert_eq!(run_count, 2, "expected user and backend r-base in run");
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
                Subdir::Linux64,
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

        let contains = |needle: &str| globs.iter().any(|g| g == needle);
        assert!(contains("DESCRIPTION"));
        assert!(contains("NAMESPACE"));
        assert!(contains("**/*.R"));
        assert!(contains("**/*.c"));
        assert!(contains("**/*.cpp"));
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

        assert!(globs.iter().any(|g| g == "inst/**/*"));
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
                Subdir::Linux64,
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
    async fn test_recommended_packages_are_dependencies() {
        let temp_dir = TempDir::new().unwrap();

        // Base packages (methods, stats) come with r-base; recommended
        // packages (MASS, Matrix) are separate packages on conda-forge
        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            r#"Package: example
Version: 1.0.0
Depends:
    R (>= 4.1.0),
    methods
Imports:
    stats,
    MASS (>= 7.3),
    Matrix
LinkingTo:
    Matrix
"#,
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-example",
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
                Subdir::Linux64,
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

        let host: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|req| req.to_string())
            .collect();
        let run: Vec<String> = generated_recipe
            .recipe
            .requirements
            .run
            .iter()
            .map(|req| req.to_string())
            .collect();

        for reqs in [&host, &run] {
            assert!(
                reqs.iter()
                    .any(|req| req.contains("r-mass") && req.contains(">=7.3")),
                "Should have r-mass >=7.3, got {reqs:?}"
            );
            assert!(
                reqs.iter().any(|req| req.starts_with("r-matrix")),
                "Should have r-matrix, got {reqs:?}"
            );
            assert!(
                !reqs
                    .iter()
                    .any(|req| req.starts_with("r-stats") || req.starts_with("r-methods")),
                "Base packages should be left out, got {reqs:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_rust_is_added_when_system_requirements_ask_for_cargo() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            r#"Package: hellorust
Version: 1.2.3
SystemRequirements: Cargo (Rust's package manager), rustc
"#,
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-hellorust",
            "version": "1.2.3",
            "targets": {
                "defaultTarget": {}
            }
        });

        let build_requirements = |platform: Subdir| {
            let project_model = project_model.clone();
            let path = temp_dir.path().to_path_buf();
            async move {
                RGenerator::default()
                    .generate_recipe(
                        &project_model,
                        &RBackendConfig::default(),
                        path,
                        platform,
                        None,
                        &HashSet::new(),
                        vec![],
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
                    .expect("Failed to generate recipe")
                    .recipe
                    .requirements
                    .build
                    .iter()
                    // The package name, without any version
                    .map(|req| {
                        req.to_string()
                            .split_whitespace()
                            .next()
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect::<Vec<String>>()
            }
        };

        let linux = build_requirements(Subdir::Linux64).await;
        assert!(
            linux.iter().any(|req| req == "rust"),
            "Should have rust in build requirements, got {linux:?}"
        );
        assert!(
            !linux.iter().any(|req| req.starts_with("rust-std")),
            "Should only add the GNU target's standard library on Windows, got {linux:?}"
        );

        let windows = build_requirements(Subdir::Win64).await;
        assert!(
            windows
                .iter()
                .any(|req| req == "rust-std-x86_64-pc-windows-gnu"),
            "Should have the GNU target's standard library on Windows, got {windows:?}"
        );
    }

    #[tokio::test]
    async fn test_cmake_and_pkg_config_are_added_when_system_requirements_name_them() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            r#"Package: demo
Version: 1.0
SystemRequirements: CMake (>= 3.15), libxml2, pkg-config
"#,
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-demo",
            "version": "1.0",
            "targets": {
                "defaultTarget": {}
            }
        });

        let build: Vec<String> = RGenerator::default()
            .generate_recipe(
                &project_model,
                &RBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await
            .expect("Failed to generate recipe")
            .recipe
            .requirements
            .build
            .iter()
            // The package name, without any version
            .map(|req| {
                req.to_string()
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();

        assert!(
            build.iter().any(|req| req == "cmake"),
            "Should have cmake in build requirements, got {build:?}"
        );
        assert!(
            build.iter().any(|req| req == "pkg-config"),
            "Should have pkg-config in build requirements, got {build:?}"
        );
    }

    #[tokio::test]
    async fn test_package_name_derived_from_description_is_r_prefixed() {
        // When the model carries no name (the inline source-dependency flow),
        // the recipe name comes from the DESCRIPTION via the metadata provider.
        // For CRAN packages, it must be r-prefixed and lowercased so that a dependent's `Imports`
        // (which maps through r_package_to_conda) resolves against it.
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: RColorBrewer\nVersion: 1.0.0\nTitle: Test Package\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
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
                Subdir::Linux64,
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

        assert_eq!(
            generated_recipe.recipe.package.name.to_string(),
            "r-rcolorbrewer"
        );
    }

    #[tokio::test]
    async fn test_package_name_derived_from_description_for_bioconductor() {
        // For Bioconductor packages, the recipe name comes from the DESCRIPTION
        // and must be bioconductor-prefixed and lowercased.
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: Rhdf5lib\nVersion: 1.0.0\nTitle: Test Package\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
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
                Subdir::Linux64,
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

        assert_eq!(
            generated_recipe.recipe.package.name.to_string(),
            "bioconductor-rhdf5lib"
        );
    }

    #[tokio::test]
    async fn test_bioconductor_dependencies_auto_mapped() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: testpkg\nVersion: 1.0.0\nImports: Biobase, GenomicRanges (>= 1.40.0), curl\n",
        )
        .await
        .unwrap();

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
                Subdir::Linux64,
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

        let host_reqs: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|r| r.to_string())
            .collect();
        let run_reqs: Vec<String> = generated_recipe
            .recipe
            .requirements
            .run
            .iter()
            .map(|r| r.to_string())
            .collect();

        // Biobase should auto-map to bioconductor-biobase
        assert!(host_reqs.iter().any(|r| r == "bioconductor-biobase"));
        assert!(run_reqs.iter().any(|r| r == "bioconductor-biobase"));

        // GenomicRanges should auto-map to bioconductor-genomicranges >=1.40.0
        assert!(
            host_reqs
                .iter()
                .any(|r| r.contains("bioconductor-genomicranges") && r.contains(">=1.40.0"))
        );
        assert!(
            run_reqs
                .iter()
                .any(|r| r.contains("bioconductor-genomicranges") && r.contains(">=1.40.0"))
        );

        // curl is a CRAN package, should map to r-curl
        assert!(host_reqs.iter().any(|r| r == "r-curl"));
        assert!(run_reqs.iter().any(|r| r == "r-curl"));
    }

    #[tokio::test]
    async fn test_explicit_dependencies_prevent_duplicates_and_conflicts() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: testpkg\nVersion: 1.0.0\nImports: Biobase\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-testpkg",
            "version": "1.0.0",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {
                        "bioconductor-biobase": {
                            "binary": {
                                "version": ">=2.60"
                            }
                        }
                    }
                }
            }
        });

        let generated_recipe = RGenerator::default()
            .generate_recipe(
                &project_model,
                &RBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Subdir::Linux64,
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

        let host_reqs: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|r| r.to_string())
            .collect();

        // Host requirements should have exactly 1 biobase spec (the user's explicit >=2.60 constraint),
        // and NO r-biobase!
        let biobase_count = host_reqs.iter().filter(|r| r.contains("biobase")).count();
        assert_eq!(biobase_count, 1);
        assert!(
            host_reqs
                .iter()
                .any(|r| r.contains("bioconductor-biobase") && r.contains(">=2.60"))
        );
        assert!(
            !host_reqs
                .iter()
                .any(|r| r == "r-biobase" || r.starts_with("r-biobase "))
        );
    }

    #[tokio::test]
    async fn test_same_named_conda_package_does_not_replace_r_package() {
        let temp_dir = TempDir::new().unwrap();

        // jose depends on the R package openssl; the package also needs the
        // openssl C library, which is a different conda package
        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: jose\nVersion: 1.2.1\nDepends: openssl (>= 1.2.1)\nImports: jsonlite\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-jose",
            "version": "1.2.1",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {
                        "openssl": {
                            "binary": {
                                "version": "*"
                            }
                        }
                    }
                }
            }
        });

        let generated_recipe = RGenerator::default()
            .generate_recipe(
                &project_model,
                &RBackendConfig::default(),
                temp_dir.path().to_path_buf(),
                Subdir::Linux64,
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

        let host: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|r| r.to_string())
            .collect();
        assert!(
            host.iter()
                .any(|r| r.starts_with("r-openssl") && r.contains(">=1.2.1")),
            "Should have r-openssl >=1.2.1 next to the openssl C library, got {host:?}"
        );
        assert!(
            host.iter().any(|r| r.starts_with("openssl")),
            "Should keep the openssl C library, got {host:?}"
        );
    }

    #[tokio::test]
    async fn test_bioconductor_dependencies_need_bioconda() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: BonEV\nVersion: 1.0\nDepends: qvalue\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-bonev",
            "version": "1.0",
            "targets": {
                "defaultTarget": {}
            }
        });
        let channel = |url: &str| ChannelUrl::from(url::Url::parse(url).unwrap());

        let generate = |channels: Vec<ChannelUrl>| {
            let project_model = project_model.clone();
            let path = temp_dir.path().to_path_buf();
            async move {
                RGenerator::default()
                    .generate_recipe(
                        &project_model,
                        &RBackendConfig::default(),
                        path,
                        Subdir::Linux64,
                        None,
                        &HashSet::new(),
                        channels,
                        None,
                        None,
                        None,
                        None,
                    )
                    .await
            }
        };

        let Err(error) = generate(vec![channel("https://prefix.dev/conda-forge")]).await else {
            panic!("Should fail without bioconda");
        };
        assert!(
            error.to_string().contains("qvalue"),
            "Should name the Bioconductor package, got {error}"
        );

        assert!(
            generate(vec![
                channel("https://prefix.dev/conda-forge"),
                channel("https://prefix.dev/bioconda"),
            ])
            .await
            .is_ok(),
            "Should work with bioconda"
        );
    }

    #[tokio::test]
    async fn test_ignore_description_dependencies() {
        let temp_dir = TempDir::new().unwrap();

        fs::write(
            temp_dir.path().join("DESCRIPTION"),
            "Package: testpkg\nVersion: 1.0.0\nImports: Biobase, curl\nLinkingTo: Rcpp\n",
        )
        .await
        .unwrap();

        let project_model = project_fixture!({
            "name": "r-testpkg",
            "version": "1.0.0",
            "targets": {
                "defaultTarget": {}
            }
        });

        let config = RBackendConfig {
            ignore_description_dependencies: Some(true),
            ..Default::default()
        };

        let generated_recipe = RGenerator::default()
            .generate_recipe(
                &project_model,
                &config,
                temp_dir.path().to_path_buf(),
                Subdir::Linux64,
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

        let host_reqs: Vec<String> = generated_recipe
            .recipe
            .requirements
            .host
            .iter()
            .map(|r| r.to_string())
            .collect();
        let run_reqs: Vec<String> = generated_recipe
            .recipe
            .requirements
            .run
            .iter()
            .map(|r| r.to_string())
            .collect();

        // Should only have r-base in host and run requirements
        assert_eq!(host_reqs, vec!["r-base".to_string()]);
        assert_eq!(run_reqs, vec!["r-base".to_string()]);
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
                Subdir::Linux64,
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
