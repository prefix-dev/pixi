mod build_script;
pub mod config;
mod distro;
mod metadata;
pub mod package_map;
pub mod package_xml;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use config::RosBackendConfig;
use fs_err as fs;
use miette::IntoDiagnostic;
use pixi_build_backend::compilers::default_compiler_variants;
use pixi_build_backend::generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams};
use pixi_build_backend::intermediate_backend::IntermediateBackendInstantiator;
use pixi_build_backend::tools::BackendIdentifier;
use rattler_build_jinja::{JinjaTemplate, Variable};
use rattler_build_recipe::stage0::{Item, Script, SerializableMatchSpec, Value};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::{ChannelUrl, Platform};

use crate::build_script::render_build_script;
use crate::config::{PackageMappingSource, extract_distro_from_channels_list};
use crate::distro::Distro;
use crate::metadata::parse_and_render;
use crate::package_map::{
    load_package_map_data, merge_conditional_lists, package_xml_to_conda_requirements,
};
use crate::package_xml::PackageXml;

#[derive(Default, Clone)]
pub struct RosGenerator {}

#[async_trait::async_trait]
impl GenerateRecipe for RosGenerator {
    type Config = RosBackendConfig;

    async fn generate_recipe(
        &self,
        model: &pixi_build_types::ProjectModel,
        config: &Self::Config,
        manifest_path: PathBuf,
        host_platform: Platform,
        _python_params: Option<PythonParams>,
        _variants: &HashSet<NormalizedKey>,
        channels: Vec<ChannelUrl>,
        _cache_dir: Option<PathBuf>,
    ) -> miette::Result<GeneratedRecipe> {
        // Determine the manifest root
        let manifest_root = if manifest_path.is_file() {
            manifest_path
                .parent()
                .ok_or_else(|| {
                    miette::miette!(
                        "Manifest path {} is a file but has no parent directory.",
                        manifest_path.display()
                    )
                })?
                .to_path_buf()
        } else {
            manifest_path.clone()
        };

        // Resolve distro from config or channels
        let distro_name = config
            .distro
            .clone()
            .or_else(|| extract_distro_from_channels_list(&channels))
            .ok_or_else(|| {
                miette::miette!(
                    "ROS distro must be either explicitly configured or \
                     auto-detected from robostack channels. \
                     A 'robostack-<distro>' channel (e.g. 'robostack-kilted') was not \
                     found in the provided channels."
                )
            })?;

        let distro = Distro::fetch(&distro_name).await?;

        // Parse package.xml
        let package_xml_path = manifest_root.join("package.xml");
        let package_xml_content = fs::read_to_string(&package_xml_path).into_diagnostic()?;

        // Set up ROS environment for condition evaluation
        let ros_version_str = if distro.is_ros1 { "1" } else { "2" };
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("ROS_DISTRO".to_string(), distro_name.clone());
        env_vars.insert("ROS_VERSION".to_string(), ros_version_str.to_string());
        if let Some(user_env) = &config.env {
            for (k, v) in user_env {
                env_vars.insert(k.clone(), v.clone());
            }
        }

        let package_xml = PackageXml::parse(&package_xml_content)
            .map(|package_xml| package_xml.evaluate_conditions(&env_vars))?;

        // Create metadata provider
        let package_mapping_files: Vec<String> = config
            .get_package_mapping_file_paths()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let extra_input_globs = config.extra_input_globs.clone().unwrap_or_default();

        let mut generated_recipe = parse_and_render(
            package_xml.clone(),
            &distro_name,
            model.clone(),
            extra_input_globs.clone(),
            package_mapping_files,
        )?;

        // Load package mappings
        let robostack_yaml: &str = include_str!("../robostack.yaml");
        let robostack_mapping: HashMap<String, package_map::PackageMapEntry> =
            serde_yaml::from_str(robostack_yaml).into_diagnostic()?;

        let mut all_mapping_sources = config.extra_package_mappings.clone();
        all_mapping_sources.push(PackageMappingSource::Mapping(robostack_mapping));

        let package_map_data = load_package_map_data(&all_mapping_sources);

        // Get requirements from package.xml
        let package_requirements = package_xml_to_conda_requirements(
            &package_xml,
            &distro,
            host_platform,
            &package_map_data,
        )?;

        // Add standard build dependencies
        let mut build_deps: Vec<&str> = vec![
            "ninja",
            "python",
            "setuptools",
            "git",
            "git-lfs",
            "cmake",
            "cpython",
        ];

        if host_platform.is_unix() {
            build_deps.extend(["patch", "make", "coreutils"]);
        }
        if host_platform.is_windows() {
            build_deps.push("m2-patch");
        }
        if host_platform.is_osx() {
            build_deps.push("tapi");
        }

        let mut build_items = package_requirements.build.clone();
        let mut host_items = package_requirements.host.clone();
        let mut run_items = package_requirements.run.clone();

        for dep in &build_deps {
            build_items.push(Item::Value(Value::new_concrete(
                SerializableMatchSpec::from(*dep),
                None,
            )));
        }

        // Add compiler dependencies
        let c_compiler =
            JinjaTemplate::new("${{ compiler('c') }}".to_string()).expect("valid jinja template");
        let cxx_compiler =
            JinjaTemplate::new("${{ compiler('cxx') }}".to_string()).expect("valid jinja template");
        build_items.push(Item::Value(Value::new_template(c_compiler, None)));
        build_items.push(Item::Value(Value::new_template(cxx_compiler, None)));

        // Add host dependencies
        let host_dep_names = ["python", "numpy", "pip", "pkg-config"];
        for dep in &host_dep_names {
            host_items.push(Item::Value(Value::new_concrete(
                SerializableMatchSpec::from(*dep),
                None,
            )));
        }

        // Add distro mutex to host and run
        let mutex_name = distro.ros_distro_mutex_name();
        host_items.push(Item::Value(Value::new_concrete(
            SerializableMatchSpec::from(mutex_name.as_str()),
            None,
        )));
        run_items.push(Item::Value(Value::new_concrete(
            SerializableMatchSpec::from(mutex_name.as_str()),
            None,
        )));

        // Merge package requirements into the model requirements
        let requirements = &mut generated_recipe.recipe.requirements;
        requirements.host = merge_conditional_lists(&requirements.host, &host_items)?;
        requirements.build = merge_conditional_lists(&requirements.build, &build_items)?;
        requirements.run = merge_conditional_lists(&requirements.run, &run_items)?;

        // Generate build script
        let build_type = package_xml.build_type();
        let build_script_content = render_build_script(&build_type, &distro_name, &manifest_root)?;

        let mut script_env: indexmap::IndexMap<String, Value<String>> = indexmap::IndexMap::new();
        script_env.insert(
            "ROS_DISTRO".to_string(),
            Value::new_concrete(distro_name.clone(), None),
        );
        script_env.insert(
            "ROS_VERSION".to_string(),
            Value::new_concrete(ros_version_str.to_string(), None),
        );
        if let Some(user_env) = &config.env {
            for (k, v) in user_env {
                script_env.insert(k.clone(), Value::new_concrete(v.clone(), None));
            }
        }

        generated_recipe.recipe.build.script =
            Script::from_content(build_script_content).with_env(script_env);

        Ok(generated_recipe)
    }

    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        editable: bool,
    ) -> miette::Result<BTreeSet<String>> {
        let mut globs: Vec<&str> = vec![
            "**/*.c",
            "**/*.cpp",
            "**/*.h",
            "**/*.hpp",
            "**/*.rs",
            "**/*.sh",
            "package.xml",
            "setup.py",
            "setup.cfg",
            "pyproject.toml",
            "Makefile",
            "CMakeLists.txt",
            "MANIFEST.in",
            "tests/**/*.py",
            "docs/**/*.rst",
            "docs/**/*.md",
            "launch/**/*.py",
            "config/*.yaml",
            "msg/**/*.msg",
            "srv/**/*.srv",
            "action/**/*.action",
        ];

        if !editable {
            globs.extend(["**/*.py", "**/*.pyx"]);
        }

        let mut result: BTreeSet<String> = globs.iter().map(|s| s.to_string()).collect();
        if let Some(extra) = &config.extra_input_globs {
            for g in extra {
                result.insert(g.clone());
            }
        }
        Ok(result)
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
        IntermediateBackendInstantiator::<RosGenerator>::new(
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

    use pixi_build_types::ProjectModel;
    use rattler_conda_types::Platform;

    use super::*;

    #[macro_export]
    macro_rules! project_fixture {
        ($($json:tt)+) => {
            serde_json::from_value::<ProjectModel>(
                serde_json::json!($($json)+)
            ).expect("Failed to create ProjectModel from JSON fixture.")
        };
    }

    fn package_xmls_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data/package_xmls")
    }

    fn default_package_map() -> HashMap<String, package_map::PackageMapEntry> {
        let content = include_str!("../robostack.yaml");
        serde_yaml::from_str(content).unwrap()
    }

    fn jazzy_distro() -> Distro {
        Distro::builder("jazzy").build()
    }

    #[test]
    fn test_package_xml_to_recipe_config() {
        let package_xml_path = package_xmls_dir().join("demo_nodes_cpp.xml");
        let content = fs::read_to_string(&package_xml_path).unwrap();
        let package_xml = PackageXml::parse(&content).unwrap();

        let env = HashMap::from([
            ("ROS_DISTRO".to_string(), "jazzy".to_string()),
            ("ROS_VERSION".to_string(), "2".to_string()),
        ]);
        let package_xml = package_xml.evaluate_conditions(&env);
        let distro = jazzy_distro();
        let package_map = default_package_map();

        let requirements = package_xml_to_conda_requirements(
            &package_xml,
            &distro,
            Platform::Linux64,
            &package_map,
        )
        .unwrap();

        insta::assert_yaml_snapshot!(requirements.build, @r###"
        - ros-jazzy-ament-cmake
        - ros-jazzy-example-interfaces
        - ros-jazzy-rcl
        - ros-jazzy-rclcpp
        - ros-jazzy-rclcpp-components
        - ros-jazzy-rcl-interfaces
        - ros-jazzy-rcpputils
        - ros-jazzy-rcutils
        - ros-jazzy-rmw
        - ros-jazzy-std-msgs
        - ros-jazzy-ament-cmake-pytest
        - ros-jazzy-ament-lint-auto
        - ros-jazzy-ament-lint-common
        - ros-jazzy-launch
        - ros-jazzy-launch-testing
        - ros-jazzy-launch-testing-ament-cmake
        - ros-jazzy-launch-testing-ros
        - ros-jazzy-ros-workspace
        "###);
        insta::assert_yaml_snapshot!(requirements.run, @r###"
        - ros-jazzy-example-interfaces
        - ros-jazzy-launch-ros
        - ros-jazzy-launch-xml
        - ros-jazzy-rcl
        - ros-jazzy-rclcpp
        - ros-jazzy-rclcpp-components
        - ros-jazzy-rcl-interfaces
        - ros-jazzy-rcpputils
        - ros-jazzy-rcutils
        - ros-jazzy-rmw
        - ros-jazzy-std-msgs
        "###);
    }

    #[test]
    fn test_ament_cmake_package_xml_to_recipe_config() {
        let package_xml_path = package_xmls_dir().join("demos_action_tutorials_interfaces.xml");
        let content = fs::read_to_string(&package_xml_path).unwrap();
        let package_xml = PackageXml::parse(&content).unwrap();

        let env = HashMap::from([
            ("ROS_DISTRO".to_string(), "jazzy".to_string()),
            ("ROS_VERSION".to_string(), "2".to_string()),
        ]);
        let package_xml = package_xml.evaluate_conditions(&env);
        let distro = jazzy_distro();
        let package_map = default_package_map();

        let requirements = package_xml_to_conda_requirements(
            &package_xml,
            &distro,
            Platform::Linux64,
            &package_map,
        )
        .unwrap();

        insta::assert_yaml_snapshot!(requirements.build, @r###"
        - ros-jazzy-ament-cmake
        - ros-jazzy-rosidl-default-generators
        - ros-jazzy-ament-lint-auto
        - ros-jazzy-ament-lint-common
        - ros-jazzy-ros-workspace
        "###);
    }

    #[tokio::test]
    async fn test_generate_recipe() {
        let package_xml_path = package_xmls_dir().join("demo_nodes_cpp.xml");
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        fs::copy(&package_xml_path, temp_path.join("package.xml")).unwrap();

        let model = project_fixture!({
            "targets": {
                "defaultTarget": {}
            }
        });

        let config = RosBackendConfig {
            distro: Some("jazzy".to_string()),
            ..Default::default()
        };

        let generator = RosGenerator::default();
        let generated_recipe = generator
            .generate_recipe(
                &model,
                &config,
                temp_path.to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe");

        assert_eq!(
            generated_recipe
                .recipe
                .package
                .name
                .as_concrete()
                .unwrap()
                .to_string(),
            "ros-jazzy-demo-nodes-cpp"
        );
        assert_eq!(
            generated_recipe
                .recipe
                .package
                .version
                .as_concrete()
                .unwrap()
                .to_string(),
            "0.37.1"
        );

        insta::assert_yaml_snapshot!(generated_recipe.recipe.requirements, @r###"
        build:
          - ros-jazzy-ament-cmake
          - ros-jazzy-example-interfaces
          - ros-jazzy-rcl
          - ros-jazzy-rclcpp
          - ros-jazzy-rclcpp-components
          - ros-jazzy-rcl-interfaces
          - ros-jazzy-rcpputils
          - ros-jazzy-rcutils
          - ros-jazzy-rmw
          - ros-jazzy-std-msgs
          - ros-jazzy-ament-cmake-pytest
          - ros-jazzy-ament-lint-auto
          - ros-jazzy-ament-lint-common
          - ros-jazzy-launch
          - ros-jazzy-launch-testing
          - ros-jazzy-launch-testing-ament-cmake
          - ros-jazzy-launch-testing-ros
          - ros-jazzy-ros-workspace
          - ninja
          - python
          - setuptools
          - git
          - git-lfs
          - cmake
          - cpython
          - patch
          - make
          - coreutils
          - "${{ compiler('c') }}"
          - "${{ compiler('cxx') }}"
        host:
          - ros-jazzy-ament-cmake
          - ros-jazzy-example-interfaces
          - ros-jazzy-rcl
          - ros-jazzy-rclcpp
          - ros-jazzy-rclcpp-components
          - ros-jazzy-rcl-interfaces
          - ros-jazzy-rcpputils
          - ros-jazzy-rcutils
          - ros-jazzy-rmw
          - ros-jazzy-std-msgs
          - ros-jazzy-ament-cmake-pytest
          - ros-jazzy-ament-lint-auto
          - ros-jazzy-ament-lint-common
          - ros-jazzy-launch
          - ros-jazzy-launch-testing
          - ros-jazzy-launch-testing-ament-cmake
          - ros-jazzy-launch-testing-ros
          - ros-jazzy-ros-workspace
          - python
          - numpy
          - pip
          - pkg-config
          - ros2-distro-mutex
        run:
          - ros-jazzy-example-interfaces
          - ros-jazzy-launch-ros
          - ros-jazzy-launch-xml
          - ros-jazzy-rcl
          - ros-jazzy-rclcpp
          - ros-jazzy-rclcpp-components
          - ros-jazzy-rcl-interfaces
          - ros-jazzy-rcpputils
          - ros-jazzy-rcutils
          - ros-jazzy-rmw
          - ros-jazzy-std-msgs
          - ros2-distro-mutex
        "###);
    }

    /// Helper to generate a recipe from a package.xml fixture.
    /// Uses Distro::fetch which requires network access.
    async fn generate_recipe_for_fixture(
        package_xml_name: &str,
        distro_name: &str,
        model: &pixi_build_types::ProjectModel,
        extra_package_mappings: Vec<PackageMappingSource>,
    ) -> GeneratedRecipe {
        let package_xmls = package_xmls_dir();
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        let source = package_xmls.join(package_xml_name);
        fs::copy(&source, temp_path.join("package.xml")).unwrap();

        let config = RosBackendConfig {
            distro: Some(distro_name.to_string()),
            extra_package_mappings,
            ..Default::default()
        };

        RosGenerator::default()
            .generate_recipe(
                model,
                &config,
                temp_path.to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe")
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_recipe_includes_project_run_dependency() {
        let model = project_fixture!({
            "name": "custom_ros",
            "version": "0.0.1",
            "description": "Demo",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {},
                    "buildDependencies": {},
                    "runDependencies": {
                        "rich": {
                            "binary": {
                                "version": ">=10.0"
                            }
                        }
                    }
                },
                "targets": {}
            }
        });

        let generated =
            generate_recipe_for_fixture("custom_ros.xml", "noetic", &model, vec![]).await;

        insta::assert_yaml_snapshot!(generated.recipe, {
            ".source[0].path" => "[path]",
            ".build.script" => "[script]",
        });
    }

    async fn generate_conditional_recipe(
        distro_name: &str,
        override_env: Option<indexmap::IndexMap<String, String>>,
    ) -> GeneratedRecipe {
        let xml = r#"<?xml version="1.0"?>
<package format="3">
  <name>conditional_pkg</name>
  <version>0.1.0</version>
  <description>Conditional dependency test</description>
  <maintainer email="test@example.com">Tester</maintainer>
  <license>MIT</license>
  <buildtool_depend condition="$ROS_VERSION == 2">ament_cmake</buildtool_depend>
  <buildtool_depend condition="$ROS_VERSION == 1">catkin</buildtool_depend>
  <build_depend condition="$ROS_VERSION == 2">rclcpp</build_depend>
  <build_depend condition="$ROS_VERSION == 1">roscpp</build_depend>
  <exec_depend condition="$ROS_VERSION == 2">rclcpp</exec_depend>
  <exec_depend condition="$ROS_VERSION == 1">roscpp</exec_depend>
</package>"#;

        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        fs::write(temp_path.join("package.xml"), xml).unwrap();

        let model = project_fixture!({
            "name": "conditional_pkg",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {},
                    "buildDependencies": {},
                    "runDependencies": {}
                },
                "targets": {}
            }
        });

        let config = RosBackendConfig {
            distro: Some(distro_name.to_string()),
            env: override_env,
            ..Default::default()
        };

        RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                temp_path.to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe")
    }

    /// Extract only the declared conditional deps (ament_cmake, catkin, rclcpp, roscpp)
    /// to make the snapshot focused.
    fn filter_conditional_deps(generated: &GeneratedRecipe, distro: &str) -> serde_json::Value {
        let declared = ["ament-cmake", "catkin", "rclcpp", "roscpp"];
        let prefix = format!("ros-{distro}-");

        let filter = |list: &rattler_build_recipe::stage0::ConditionalList<
            SerializableMatchSpec,
        >|
         -> Vec<String> {
            let mut names: Vec<String> = list
                .iter()
                .filter_map(|item| match item {
                    Item::Value(v) => v.as_concrete().map(|s| s.to_string()),
                    _ => None,
                })
                .filter(|dep| {
                    dep.starts_with(&prefix)
                        && declared
                            .iter()
                            .any(|d| dep.strip_prefix(&prefix) == Some(*d))
                })
                .collect();
            names.sort();
            names
        };

        serde_json::json!({
            "build": filter(&generated.recipe.requirements.build),
            "run": filter(&generated.recipe.requirements.run),
        })
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_condition_evaluation_ros2_default() {
        let generated = generate_conditional_recipe("jazzy", None).await;
        insta::assert_yaml_snapshot!(filter_conditional_deps(&generated, "jazzy"), @r###"
        build:
          - ros-jazzy-ament-cmake
          - ros-jazzy-rclcpp
        run:
          - ros-jazzy-rclcpp
        "###);
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_condition_evaluation_ros1_default() {
        let generated = generate_conditional_recipe("noetic", None).await;
        insta::assert_yaml_snapshot!(filter_conditional_deps(&generated, "noetic"), @r###"
        build:
          - ros-noetic-catkin
          - ros-noetic-roscpp
        run:
          - ros-noetic-roscpp
        "###);
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_condition_evaluation_ros2_override_to_ros1() {
        let env = indexmap::IndexMap::from([
            ("ROS_VERSION".to_string(), "1".to_string()),
            ("ROS_DISTRO".to_string(), "custom-jazzy".to_string()),
        ]);
        let generated = generate_conditional_recipe("jazzy", Some(env)).await;
        insta::assert_yaml_snapshot!(filter_conditional_deps(&generated, "jazzy"), @r###"
        build:
          - ros-jazzy-catkin
          - ros-jazzy-roscpp
        run:
          - ros-jazzy-roscpp
        "###);
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_generate_recipe_with_versions() {
        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        let test_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");
        let generated = generate_recipe_for_fixture(
            "version_constraints.xml",
            "noetic",
            &model,
            vec![PackageMappingSource::File {
                path: test_data.join("other_package_map.yaml"),
            }],
        )
        .await;

        insta::assert_yaml_snapshot!(generated.recipe, {
            ".source[0].path" => "[path]",
            ".build.script" => "[script]",
        });
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_generate_recipe_with_mutex_version() {
        let model = project_fixture!({
            "name": "custom_ros",
            "version": "0.0.1",
            "description": "Demo",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {
                        "ros-distro-mutex": {
                            "binary": {
                                "version": "0.5.*"
                            }
                        }
                    },
                    "buildDependencies": {},
                    "runDependencies": {
                        "rich": {
                            "binary": {
                                "version": ">=10.0"
                            }
                        }
                    }
                },
                "targets": {}
            }
        });

        let test_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");
        let generated = generate_recipe_for_fixture(
            "version_constraints.xml",
            "noetic",
            &model,
            vec![PackageMappingSource::File {
                path: test_data.join("other_package_map.yaml"),
            }],
        )
        .await;

        insta::assert_yaml_snapshot!(generated.recipe, {
            ".source[0].path" => "[path]",
            ".build.script" => "[script]",
        });
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_generate_recipe_with_versions_in_model_and_package() {
        let model = project_fixture!({
            "name": "custom_ros",
            "version": "0.0.1",
            "description": "Demo",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": {},
                    "buildDependencies": {},
                    "runDependencies": {
                        "asio": {
                            "binary": {
                                "version": ">=9.0"
                            }
                        }
                    }
                },
                "targets": {}
            }
        });

        let test_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");
        let generated = generate_recipe_for_fixture(
            "version_constraints.xml",
            "noetic",
            &model,
            vec![PackageMappingSource::File {
                path: test_data.join("other_package_map.yaml"),
            }],
        )
        .await;

        insta::assert_yaml_snapshot!(generated.recipe, {
            ".source[0].path" => "[path]",
            ".build.script" => "[script]",
        });
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_generate_recipe_with_explicit_package_xml_path() {
        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        let package_xmls = package_xmls_dir();
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();

        let source = package_xmls.join("version_constraints.xml");
        let dest = temp_path.join("package.xml");
        fs::copy(&source, &dest).unwrap();

        let test_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");
        let config = RosBackendConfig {
            distro: Some("noetic".to_string()),
            extra_package_mappings: vec![PackageMappingSource::File {
                path: test_data.join("other_package_map.yaml"),
            }],
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                dest.clone(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await
            .expect("Failed to generate recipe with explicit package.xml path");

        insta::assert_yaml_snapshot!(generated.recipe, {
            ".source[0].path" => "[path]",
            ".build.script" => "[script]",
        });
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_config_auto_detects_distro_from_channel() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let source = package_xmls_dir().join("demo_nodes_cpp.xml");
        fs::copy(&source, temp_path.join("package.xml")).unwrap();

        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        // No distro in config, but jazzy channel provided
        let config = RosBackendConfig::default();

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                temp_path.to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![ChannelUrl::from(
                    url::Url::parse("https://prefix.dev/robostack-jazzy").unwrap(),
                )],
                None,
            )
            .await
            .expect("Should auto-detect distro from channel");

        assert_eq!(
            generated
                .recipe
                .package
                .name
                .as_concrete()
                .unwrap()
                .to_string(),
            "ros-jazzy-demo-nodes-cpp"
        );
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_config_explicit_distro_overrides_channel() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let source = package_xmls_dir().join("demo_nodes_cpp.xml");
        fs::copy(&source, temp_path.join("package.xml")).unwrap();

        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        // Explicit distro "humble" should win over channel "robostack-jazzy"
        let config = RosBackendConfig {
            distro: Some("humble".to_string()),
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                temp_path.to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![ChannelUrl::from(
                    url::Url::parse("https://prefix.dev/robostack-jazzy").unwrap(),
                )],
                None,
            )
            .await
            .expect("Explicit distro should override channel");

        assert_eq!(
            generated
                .recipe
                .package
                .name
                .as_concrete()
                .unwrap()
                .to_string(),
            "ros-humble-demo-nodes-cpp"
        );
    }

    #[tokio::test]
    async fn test_config_fails_without_distro_or_channel() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let source = package_xmls_dir().join("demo_nodes_cpp.xml");
        fs::copy(&source, temp_path.join("package.xml")).unwrap();

        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        // No distro, no robostack channel -> should fail
        let config = RosBackendConfig::default();

        let result = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                temp_path.to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![ChannelUrl::from(
                    url::Url::parse("https://prefix.dev/conda-forge").unwrap(),
                )],
                None,
            )
            .await;

        let err = result
            .err()
            .expect("Should fail when distro cannot be determined");
        assert!(
            err.to_string().contains("ROS distro must be either"),
            "Error should mention distro auto-detection, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_config_fails_without_channels() {
        let temp_dir = tempfile::tempdir().unwrap();
        let temp_path = temp_dir.path();
        let source = package_xmls_dir().join("demo_nodes_cpp.xml");
        fs::copy(&source, temp_path.join("package.xml")).unwrap();

        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        // No distro, no channels at all -> should fail
        let config = RosBackendConfig::default();

        let result = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                temp_path.to_path_buf(),
                Platform::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
            )
            .await;

        assert!(result.is_err(), "Should fail when no channels provided");
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_generate_recipe_with_custom_ros() {
        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        let test_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");
        let generated = generate_recipe_for_fixture(
            "custom_ros.xml",
            "noetic",
            &model,
            vec![PackageMappingSource::File {
                path: test_data.join("other_package_map.yaml"),
            }],
        )
        .await;

        assert_eq!(
            generated
                .recipe
                .package
                .name
                .as_concrete()
                .unwrap()
                .to_string(),
            "ros-noetic-custom-ros"
        );

        let run_deps: Vec<String> = generated
            .recipe
            .requirements
            .run
            .iter()
            .filter_map(|item| match item {
                Item::Value(v) => v.as_concrete().map(|s| s.to_string()),
                _ => None,
            })
            .collect();

        // custom_ros.xml has <depend>ros_package</depend> which maps to
        // ros: [ros_package, ros_package_msgs] in other_package_map.yaml
        assert!(
            run_deps.iter().any(|d| d == "ros-noetic-ros-package"),
            "Expected ros-noetic-ros-package in run deps: {run_deps:?}"
        );
        assert!(
            run_deps.iter().any(|d| d == "ros-noetic-ros-package-msgs"),
            "Expected ros-noetic-ros-package-msgs in run deps: {run_deps:?}"
        );
        // multi_package maps to conda: [multi-package-a, multi-package-b]
        assert!(
            run_deps.iter().any(|d| d == "multi-package-a"),
            "Expected multi-package-a in run deps: {run_deps:?}"
        );
        assert!(
            run_deps.iter().any(|d| d == "multi-package-b"),
            "Expected multi-package-b in run deps: {run_deps:?}"
        );
    }

    #[tokio::test]
    #[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
    async fn test_generate_recipe_with_inline_package_mappings() {
        let model = project_fixture!({
            "targets": { "defaultTarget": {} }
        });

        let test_data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_data");

        // Inline mapping overrides ros_package to custom values
        let mut inline_map: HashMap<String, package_map::PackageMapEntry> = HashMap::new();
        let mut ros_entry = package_map::PackageMapEntry::new();
        ros_entry.insert(
            "ros".to_string(),
            package_map::PlatformMapping::List(vec![
                "ros-custom2".to_string(),
                "ros-custom2-msgs".to_string(),
            ]),
        );
        inline_map.insert("ros_package".to_string(), ros_entry);

        let generated = generate_recipe_for_fixture(
            "custom_ros.xml",
            "noetic",
            &model,
            vec![
                PackageMappingSource::Mapping(inline_map),
                PackageMappingSource::File {
                    path: test_data.join("other_package_map.yaml"),
                },
            ],
        )
        .await;

        let run_deps: Vec<String> = generated
            .recipe
            .requirements
            .run
            .iter()
            .filter_map(|item| match item {
                Item::Value(v) => v.as_concrete().map(|s| s.to_string()),
                _ => None,
            })
            .collect();

        assert!(
            run_deps.iter().any(|d| d == "ros-noetic-ros-custom2"),
            "Expected ros-noetic-ros-custom2 in run deps: {run_deps:?}"
        );
        assert!(
            run_deps.iter().any(|d| d == "ros-noetic-ros-custom2-msgs"),
            "Expected ros-noetic-ros-custom2-msgs in run deps: {run_deps:?}"
        );
    }
}
