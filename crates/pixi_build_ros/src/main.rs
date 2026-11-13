use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs_err as fs;
use miette::IntoDiagnostic;
use pixi_build_backend::compilers::default_compiler_variants;
use pixi_build_backend::generated_recipe::{GenerateRecipe, GeneratedRecipe, PythonParams};
use pixi_build_backend::intermediate_backend::IntermediateBackendInstantiator;
use pixi_build_backend::tools::BackendIdentifier;
use pixi_build_ros::config::RosBackendConfig;
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{Script, SingleOutputRecipe, Value};
use rattler_build_types::NormalizedKey;
use rattler_conda_types::{ChannelUrl, Subdir};

use pixi_build_ros::build_script::{BuildMode, BuildType, ScriptVars, render_build_script};
use pixi_build_ros::config::{PackageMappingSource, extract_distro_from_channels_list};
use pixi_build_ros::distro::Distro;
use pixi_build_ros::metadata::parse_and_render;
use pixi_build_ros::package_map::{
    load_package_map_data, merge_conditional_lists, package_xml_to_conda_requirements,
};
use pixi_build_ros::package_xml::PackageXml;
use pixi_build_ros::requirements::Rules;
use pixi_build_ros::{package_map, workspace_discovery};

#[derive(Default, Clone)]
pub struct RosGenerator {}

#[async_trait::async_trait]
impl GenerateRecipe for RosGenerator {
    type Config = RosBackendConfig;
    type Recipe = SingleOutputRecipe;

    #[tracing::instrument(
        name = "ros_generate_recipe",
        skip_all,
        fields(
            manifest_path = %manifest_path.display(),
            workspace = workspace_directory.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "<none>".to_string()),
        ),
    )]
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
        workspace_directory: Option<PathBuf>,
        checkout_root: Option<PathBuf>,
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

        let distro = Distro::new(distro_name);

        // Parse package.xml
        let package_xml_path = manifest_root.join("package.xml");
        let package_xml_content = fs::read_to_string(&package_xml_path).into_diagnostic()?;

        // Set up ROS environment for condition evaluation
        let mut env_vars: HashMap<String, String> = HashMap::new();
        env_vars.insert("ROS_DISTRO".to_string(), distro.name.clone());
        env_vars.insert(
            "ROS_VERSION".to_string(),
            distro.version.as_env_value().to_string(),
        );
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
            &distro,
            model.clone(),
            extra_input_globs.clone(),
            package_mapping_files,
        )?;

        // `parse_and_render` already populated `metadata_input_globs` with
        // the provider's anchored package-local patterns (`setup.py`,
        // `CMakeLists.txt`, ...).  Workspace-discovery globs (the
        // `../**/package.xml` / `**/COLCON_IGNORE` family and the
        // `!**/.*/**` hidden-folder exclusion) are no longer emitted on
        // the flat list: they're carried by `metadata_input_glob_sets`
        // with marker semantics that the flat form can't express.

        // Load package mappings: the user's over the bundled RoboStack ones
        let mut all_mapping_sources = config.extra_package_mappings.clone();
        all_mapping_sources.push(PackageMappingSource::Mapping(
            package_map::bundled_package_map(&distro.name)?,
        ));

        let package_map_data = load_package_map_data(&all_mapping_sources)?;

        // Get requirements from package.xml, by RoboStack's rules
        let build_type = BuildType::of(&package_xml, &distro)?;
        let rules = Rules {
            // `stdlib('c')` only renders with a `c_stdlib` variant
            c_stdlib: variants.contains(&NormalizedKey::from("c_stdlib")),
            ..Rules::new(distro.clone(), host_platform)
        };
        let mut package_requirements =
            package_xml_to_conda_requirements(&package_xml, build_type, &rules, &package_map_data)?;

        // Discover sibling ROS packages in the workspace and rewrite any
        // package.xml deps that match a sibling into source dependencies, so
        // pixi resolves them against the sibling directory instead of looking
        // them up as a binary through RoboStack.
        //
        // Prefer `checkout_root` (set by newer pixi versions) when available
        // because it correctly resolves to the git/url checkout root for
        // remote source dependencies — `workspace_directory` for those
        // cases points at the package's subdirectory and would miss every
        // sibling.  For path sources the two values agree.  Older pixi
        // versions don't send `checkout_root`; fall back to
        // `workspace_directory` so this backend keeps working there.
        let discovery_root = checkout_root.as_deref().or(workspace_directory.as_deref());
        if let Some(workspace_root) = discovery_root
            && !config.ignore_workspace_sources
        {
            let discovery = workspace_discovery::discover_ros_packages(workspace_root)?;

            // Emit the structured form.  Pointing `root` at the workspace
            // lets pixi walk from there directly without ascending via
            // `../..` patterns; the marker semantics handle pruning at
            // `COLCON_IGNORE` / `AMENT_IGNORE` / `CATKIN_IGNORE`.
            let mut structured = discovery.input_glob_set;
            structured.root = Some(workspace_root.to_path_buf());
            generated_recipe.metadata_input_glob_sets.push(structured);

            let sibling_specs = workspace_discovery::sibling_source_spec_map(
                &discovery.packages,
                &package_xml.name,
                &manifest_root,
                &distro,
            );

            // Apply per-class: a manual entry in the model for the same conda
            // name suppresses discovery's override for that class only.
            let build_overrides = workspace_discovery::filter_unspecified(
                &sibling_specs,
                &generated_recipe.recipe.requirements.build,
            );
            let host_overrides = workspace_discovery::filter_unspecified(
                &sibling_specs,
                &generated_recipe.recipe.requirements.host,
            );
            let run_overrides = workspace_discovery::filter_unspecified(
                &sibling_specs,
                &generated_recipe.recipe.requirements.run,
            );

            package_requirements.build = workspace_discovery::apply_sibling_overrides(
                package_requirements.build,
                &build_overrides,
            );
            package_requirements.host = workspace_discovery::apply_sibling_overrides(
                package_requirements.host,
                &host_overrides,
            );
            package_requirements.run = workspace_discovery::apply_sibling_overrides(
                package_requirements.run,
                &run_overrides,
            );
        }

        // Mirror the provider's package-local literals (`setup.py`,
        // `CMakeLists.txt`, ...) into the structured list as a second
        // group rooted at the package manifest (the consumer's default).
        if !generated_recipe.metadata_input_globs.is_empty() {
            generated_recipe
                .metadata_input_glob_sets
                .push(pixi_build_types::InputGlobSet {
                    patterns: generated_recipe.metadata_input_globs.clone(),
                    markers: Vec::new(),
                    exclude_hidden: true,
                    root: None,
                });
        }

        // Merge package requirements into the model requirements
        let requirements = &mut generated_recipe.recipe.requirements;
        requirements.host =
            merge_conditional_lists(&requirements.host, &package_requirements.host)?;
        requirements.build =
            merge_conditional_lists(&requirements.build, &package_requirements.build)?;
        requirements.run = merge_conditional_lists(&requirements.run, &package_requirements.run)?;

        // Generate the build script, which runs on the build machine
        let windows = Subdir::current().unwrap_or(Subdir::NoArch).is_windows();
        let source_dir = manifest_root.display().to_string();
        let build_script_content = render_build_script(
            build_type,
            windows,
            BuildMode::Incremental,
            &ScriptVars {
                generator: "pixi-build-ros",
                source_dir: &source_dir,
                package_prefix: &distro.package_prefix(),
                distro: &distro.name,
                cmake_args: "",
            },
        );

        let mut script_env: indexmap::IndexMap<String, Value<String>> = indexmap::IndexMap::new();
        script_env.insert(
            "ROS_DISTRO".to_string(),
            Value::new_concrete(distro.name.clone(), None),
        );
        script_env.insert(
            "ROS_VERSION".to_string(),
            Value::new_concrete(distro.version.as_env_value().to_string(), None),
        );
        if let Some(user_env) = &config.env {
            for (k, v) in user_env {
                script_env.insert(k.clone(), Value::new_concrete(v.clone(), None));
            }
        }

        *generated_recipe
            .recipe
            .build
            .plan
            .script_mut()
            .expect("generated recipes use script mode") =
            Script::from_content(build_script_content)
                .with_env(script_env)
                .with_secrets(model.secrets.iter().cloned().collect());

        Ok(generated_recipe)
    }

    fn extract_input_globs_from_build(
        &self,
        config: &Self::Config,
        _workdir: impl AsRef<Path>,
        _editable: bool,
    ) -> miette::Result<Vec<String>> {
        let globs: Vec<&str> = vec![
            "**/*.c",
            "**/*.cpp",
            "**/*.h",
            "**/*.hpp",
            "**/*.rs",
            "**/*.sh",
            "**/*.py",
            "**/*.pyx",
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

        let mut result: Vec<String> = globs.iter().map(|s| s.to_string()).collect();
        if let Some(extra) = &config.extra_input_globs {
            result.extend(extra.iter().cloned());
        }
        Ok(result)
    }

    fn default_variants(
        &self,
        host_platform: Subdir,
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
    use rattler_conda_types::Subdir;

    use rattler_build_recipe::stage0::{Item, SerializableMatchSpec};

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
        package_map::bundled_package_map("jazzy").unwrap()
    }

    fn jazzy_distro() -> Distro {
        Distro::new("jazzy")
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

        let build_type = BuildType::of(&package_xml, &distro).unwrap();
        let rules = Rules::new(distro, Subdir::Linux64);
        let requirements =
            package_xml_to_conda_requirements(&package_xml, build_type, &rules, &package_map)
                .unwrap();

        insta::assert_yaml_snapshot!(requirements.host, @"
        - numpy
        - pip
        - python
        - ros2-ament-cmake
        - ros2-ament-cmake-pytest
        - ros2-ament-lint-auto
        - ros2-ament-lint-common
        - ros2-distro-mutex * jazzy_*
        - ros2-example-interfaces
        - ros2-launch
        - ros2-launch-testing
        - ros2-launch-testing-ament-cmake
        - ros2-launch-testing-ros
        - ros2-rcl
        - ros2-rcl-interfaces
        - ros2-rclcpp
        - ros2-rclcpp-components
        - ros2-rcpputils
        - ros2-rcutils
        - ros2-rmw
        - ros2-ros-environment
        - ros2-ros-workspace
        - ros2-std-msgs
        - if: build_platform == target_platform
          then: pkg-config
        ");
        insta::assert_yaml_snapshot!(requirements.run, @"
        - python
        - ros2-distro-mutex * jazzy_*
        - ros2-example-interfaces
        - ros2-launch-ros
        - ros2-launch-xml
        - ros2-rcl
        - ros2-rcl-interfaces
        - ros2-rclcpp
        - ros2-rclcpp-components
        - ros2-rcpputils
        - ros2-rcutils
        - ros2-rmw
        - ros2-ros-workspace
        - ros2-std-msgs
        ");
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

        let build_type = BuildType::of(&package_xml, &distro).unwrap();
        let rules = Rules::new(distro, Subdir::Linux64);
        let requirements =
            package_xml_to_conda_requirements(&package_xml, build_type, &rules, &package_map)
                .unwrap();

        insta::assert_yaml_snapshot!(requirements.host, @"
        - numpy
        - pip
        - python
        - ros2-ament-cmake
        - ros2-ament-lint-auto
        - ros2-ament-lint-common
        - ros2-distro-mutex * jazzy_*
        - ros2-ros-environment
        - ros2-ros-workspace
        - ros2-rosidl-default-generators
        - if: build_platform == target_platform
          then: pkg-config
        ");
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
            generated_recipe
                .recipe
                .package
                .name
                .as_concrete()
                .unwrap()
                .to_string(),
            "ros2-demo-nodes-cpp"
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

        insta::assert_yaml_snapshot!(generated_recipe.recipe.requirements, @r#"
        build:
          - "${{ compiler('cxx') }}"
          - "${{ compiler('c') }}"
          - ninja
          - python
          - setuptools
          - git
          - git-lfs
          - patch
          - make
          - coreutils
          - cmake
          - cython
          - if: build_platform != target_platform
            then: pkg-config
          - if: build_platform != target_platform
            then: python
          - if: build_platform != target_platform
            then: "cross-python_${{ target_platform }}"
          - if: build_platform != target_platform
            then: numpy
        host:
          - numpy
          - pip
          - python
          - ros2-ament-cmake
          - ros2-ament-cmake-pytest
          - ros2-ament-lint-auto
          - ros2-ament-lint-common
          - ros2-distro-mutex * jazzy_*
          - ros2-example-interfaces
          - ros2-launch
          - ros2-launch-testing
          - ros2-launch-testing-ament-cmake
          - ros2-launch-testing-ros
          - ros2-rcl
          - ros2-rcl-interfaces
          - ros2-rclcpp
          - ros2-rclcpp-components
          - ros2-rcpputils
          - ros2-rcutils
          - ros2-rmw
          - ros2-ros-environment
          - ros2-ros-workspace
          - ros2-std-msgs
          - if: build_platform == target_platform
            then: pkg-config
        run:
          - python
          - ros2-distro-mutex * jazzy_*
          - ros2-example-interfaces
          - ros2-launch-ros
          - ros2-launch-xml
          - ros2-rcl
          - ros2-rcl-interfaces
          - ros2-rclcpp
          - ros2-rclcpp-components
          - ros2-rcpputils
          - ros2-rcutils
          - ros2-rmw
          - ros2-ros-workspace
          - ros2-std-msgs
        "#);
    }

    /// Helper to generate a recipe from a package.xml fixture.
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
    }

    #[tokio::test]
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
    }

    /// Extract only the declared conditional deps (ament_cmake, catkin, rclcpp, roscpp)
    /// to make the snapshot focused.
    fn filter_conditional_deps(generated: &GeneratedRecipe, distro: &str) -> serde_json::Value {
        let declared = ["ament-cmake", "catkin", "rclcpp", "roscpp"];
        let prefix = format!("{}-", Distro::new(distro).package_prefix());

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
            "host": filter(&generated.recipe.requirements.host),
            "run": filter(&generated.recipe.requirements.run),
        })
    }

    #[tokio::test]
    async fn test_condition_evaluation_ros2_default() {
        let generated = generate_conditional_recipe("jazzy", None).await;
        insta::assert_yaml_snapshot!(filter_conditional_deps(&generated, "jazzy"), @"
        host:
          - ros2-ament-cmake
          - ros2-rclcpp
        run:
          - ros2-rclcpp
        ");
    }

    #[tokio::test]
    async fn test_condition_evaluation_ros1_default() {
        let generated = generate_conditional_recipe("noetic", None).await;
        insta::assert_yaml_snapshot!(filter_conditional_deps(&generated, "noetic"), @"
        host:
          - ros-noetic-catkin
          - ros-noetic-roscpp
        run:
          - ros-noetic-roscpp
        ");
    }

    #[tokio::test]
    async fn test_condition_evaluation_ros2_override_to_ros1() {
        let env = indexmap::IndexMap::from([
            ("ROS_VERSION".to_string(), "1".to_string()),
            ("ROS_DISTRO".to_string(), "custom-jazzy".to_string()),
        ]);
        let generated = generate_conditional_recipe("jazzy", Some(env)).await;
        insta::assert_yaml_snapshot!(filter_conditional_deps(&generated, "jazzy"), @"
        host:
          - ros2-catkin
          - ros2-roscpp
        run:
          - ros2-roscpp
        ");
    }

    #[tokio::test]
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
            .expect("Failed to generate recipe with explicit package.xml path");

        insta::assert_yaml_snapshot!(generated.recipe, {
            ".source[0].path" => "[path]",
            ".build.script" => "[script]",
        });
    }

    #[tokio::test]
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
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![ChannelUrl::from(
                    url::Url::parse("https://prefix.dev/robostack-jazzy").unwrap(),
                )],
                None,
                None,
                None,
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
            "ros2-demo-nodes-cpp"
        );
    }

    #[tokio::test]
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
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![ChannelUrl::from(
                    url::Url::parse("https://prefix.dev/robostack-jazzy").unwrap(),
                )],
                None,
                None,
                None,
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
            "ros2-demo-nodes-cpp"
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
                Subdir::Linux64,
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
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                None,
                None,
            )
            .await;

        assert!(result.is_err(), "Should fail when no channels provided");
    }

    #[tokio::test]
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

    fn write_minimal_package_xml(dir: &std::path::Path, name: &str, deps: &[&str]) {
        fs::create_dir_all(dir).unwrap();
        let depend_tags: String = deps
            .iter()
            .map(|d| format!("  <depend>{d}</depend>\n"))
            .collect();
        let xml = format!(
            r#"<?xml version="1.0"?>
<package format="3">
  <name>{name}</name>
  <version>0.0.1</version>
  <description>test</description>
  <maintainer email="test@example.com">Tester</maintainer>
  <license>MIT</license>
  <buildtool_depend>ament_cmake</buildtool_depend>
{depend_tags}</package>
"#
        );
        fs::write(dir.join("package.xml"), xml).unwrap();
    }

    /// Returns the first concrete item whose package name equals `conda_name`.
    fn find_concrete<'a>(
        list: &'a rattler_build_recipe::stage0::ConditionalList<SerializableMatchSpec>,
        conda_name: &str,
    ) -> Option<&'a SerializableMatchSpec> {
        list.iter().find_map(|item| match item {
            Item::Value(v) => v.as_concrete().filter(|s| {
                s.0.name
                    .as_exact()
                    .map(|n| n.as_normalized() == conda_name)
                    .unwrap_or(false)
            }),
            _ => None,
        })
    }

    #[tokio::test]
    async fn test_workspace_discovery_emits_source_dep_for_sibling() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path();

        write_minimal_package_xml(
            &workspace_root.join("src").join("pkg_a"),
            "pkg_a",
            &["pkg_b"],
        );
        write_minimal_package_xml(&workspace_root.join("src").join("pkg_b"), "pkg_b", &[]);

        let pkg_a_manifest = workspace_root.join("src").join("pkg_a");

        let model = project_fixture!({ "targets": { "defaultTarget": {} } });
        let config = RosBackendConfig {
            distro: Some("jazzy".to_string()),
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                pkg_a_manifest.clone(),
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                Some(workspace_root.to_path_buf()),
                None,
            )
            .await
            .expect("generate_recipe should succeed");

        let sibling_in_host = find_concrete(&generated.recipe.requirements.host, "ros2-pkg-b")
            .expect("sibling should appear in host requirements");
        assert!(
            sibling_in_host.0.url.is_some(),
            "expected ros2-pkg-b to be a source dep, got: {sibling_in_host}"
        );

        let sibling_in_run = find_concrete(&generated.recipe.requirements.run, "ros2-pkg-b")
            .expect("sibling should also appear in run requirements");
        assert!(
            sibling_in_run.0.url.is_some(),
            "expected ros2-pkg-b in run to be source, got: {sibling_in_run}"
        );
    }

    #[tokio::test]
    async fn test_workspace_discovery_opt_out_falls_back_to_binary() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path();
        write_minimal_package_xml(&workspace_root.join("pkg_a"), "pkg_a", &["pkg_b"]);
        write_minimal_package_xml(&workspace_root.join("pkg_b"), "pkg_b", &[]);

        let model = project_fixture!({ "targets": { "defaultTarget": {} } });
        let config = RosBackendConfig {
            distro: Some("jazzy".to_string()),
            ignore_workspace_sources: true,
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                workspace_root.join("pkg_a"),
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                Some(workspace_root.to_path_buf()),
                None,
            )
            .await
            .expect("generate_recipe should succeed");

        let sibling = find_concrete(&generated.recipe.requirements.host, "ros2-pkg-b")
            .expect("sibling should still be present as binary");
        assert!(
            sibling.0.url.is_none(),
            "opt-out should keep binary spec, got source: {sibling}"
        );
    }

    #[tokio::test]
    async fn test_workspace_discovery_yields_to_manual_model_entry() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path();
        write_minimal_package_xml(&workspace_root.join("pkg_a"), "pkg_a", &["pkg_b"]);
        write_minimal_package_xml(&workspace_root.join("pkg_b"), "pkg_b", &[]);

        // Model declares ros2-pkg-b in run only, with an explicit version.
        // Discovery must not override that entry, but build/host still get the
        // source dep because the manual entry was per-class.
        let model = project_fixture!({
            "targets": {
                "defaultTarget": {
                    "runDependencies": {
                        "ros2-pkg-b": { "binary": { "version": "==1.2.3" } }
                    }
                }
            }
        });
        let config = RosBackendConfig {
            distro: Some("jazzy".to_string()),
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                workspace_root.join("pkg_a"),
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                Some(workspace_root.to_path_buf()),
                None,
            )
            .await
            .expect("generate_recipe should succeed");

        let in_run = find_concrete(&generated.recipe.requirements.run, "ros2-pkg-b")
            .expect("run entry should remain");
        assert!(
            in_run.0.url.is_none(),
            "manual model entry must stay binary, got source: {in_run}"
        );

        let in_host = find_concrete(&generated.recipe.requirements.host, "ros2-pkg-b")
            .expect("host entry should exist");
        assert!(
            in_host.0.url.is_some(),
            "host override should still kick in, got binary: {in_host}"
        );
    }

    #[tokio::test]
    async fn test_workspace_discovery_globs_injected_into_metadata() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path();
        write_minimal_package_xml(&workspace_root.join("src").join("pkg_a"), "pkg_a", &[]);
        fs::create_dir_all(workspace_root.join("build")).unwrap();
        fs::write(workspace_root.join("build").join("COLCON_IGNORE"), b"").unwrap();

        let model = project_fixture!({ "targets": { "defaultTarget": {} } });
        let config = RosBackendConfig {
            distro: Some("jazzy".to_string()),
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                workspace_root.join("src").join("pkg_a"),
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                Some(workspace_root.to_path_buf()),
                None,
            )
            .await
            .expect("generate_recipe should succeed");

        // The flat `metadata_input_globs` carries only the provider's
        // anchored package-local literals.  Workspace discovery patterns
        // are now expressed structurally via `metadata_input_glob_sets`.
        let flat: Vec<String> = generated.metadata_input_globs.to_vec();
        assert!(
            flat.iter().any(|g| g == "package.xml"),
            "expected provider glob 'package.xml' in flat list, got: {flat:?}"
        );
        assert!(
            !flat.iter().any(|g| g.contains("COLCON_IGNORE")),
            "workspace discovery patterns should not appear in flat list, got: {flat:?}"
        );
        assert!(
            !flat
                .iter()
                .any(|g| g == "!**/.*/**" || g.ends_with("/!**/.*/**")),
            "hidden-folder exclusion should not appear in flat list, got: {flat:?}"
        );

        // The structured list carries the workspace discovery (with markers)
        // plus a second group for the provider literals.
        let groups = &generated.metadata_input_glob_sets;
        assert!(
            groups
                .iter()
                .any(|g| g.markers.iter().any(|m| m == "COLCON_IGNORE")),
            "expected a structured group with COLCON_IGNORE as a marker, got: {groups:?}"
        );
    }

    #[tokio::test]
    async fn test_workspace_discovery_skips_pkg_under_colcon_ignore() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path();
        write_minimal_package_xml(&workspace_root.join("pkg_a"), "pkg_a", &["pkg_b"]);
        // pkg_b is buried under a COLCON_IGNORE'd directory, so discovery should
        // skip it and the recipe must fall back to RoboStack (binary).
        write_minimal_package_xml(&workspace_root.join("vendor").join("pkg_b"), "pkg_b", &[]);
        fs::write(workspace_root.join("vendor").join("COLCON_IGNORE"), b"").unwrap();

        let model = project_fixture!({ "targets": { "defaultTarget": {} } });
        let config = RosBackendConfig {
            distro: Some("jazzy".to_string()),
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                workspace_root.join("pkg_a"),
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                Some(workspace_root.to_path_buf()),
                None,
            )
            .await
            .expect("generate_recipe should succeed");

        let sibling = find_concrete(&generated.recipe.requirements.host, "ros2-pkg-b")
            .expect("ignored sibling should still appear via RoboStack fallback");
        assert!(
            sibling.0.url.is_none(),
            "ignored sibling must not be turned into a source dep: {sibling}"
        );
    }

    #[tokio::test]
    async fn test_workspace_discovery_does_not_add_self_as_source() {
        let workspace = tempfile::tempdir().unwrap();
        let workspace_root = workspace.path();
        write_minimal_package_xml(&workspace_root.join("pkg_a"), "pkg_a", &[]);

        let model = project_fixture!({ "targets": { "defaultTarget": {} } });
        let config = RosBackendConfig {
            distro: Some("jazzy".to_string()),
            ..Default::default()
        };

        let generated = RosGenerator::default()
            .generate_recipe(
                &model,
                &config,
                workspace_root.join("pkg_a"),
                Subdir::Linux64,
                None,
                &HashSet::new(),
                vec![],
                None,
                None,
                Some(workspace_root.to_path_buf()),
                None,
            )
            .await
            .expect("generate_recipe should succeed");

        let self_in_host = find_concrete(&generated.recipe.requirements.host, "ros2-pkg-a");
        assert!(
            self_in_host.is_none(),
            "current package must not list itself as a source dep: {self_in_host:?}"
        );
    }
}
