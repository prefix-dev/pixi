//! Pixi-native code path: generate a recipe directly from `pixi.toml`,
//! without any `package.xml`, rosdep mapping, or rosdistro fetch.
//!
//! Mode is selected by `RosMode::PixiNative` (or auto-detected when no
//! `package.xml` is present alongside the manifest).

use std::collections::BTreeSet;
use std::path::PathBuf;

use indexmap::IndexMap;
use miette::Diagnostic;
use pixi_build_backend::generated_recipe::{
    DefaultMetadataProvider, GeneratedRecipe,
};
use pixi_build_types::{ProjectModel, Target};
use rattler_build_jinja::JinjaTemplate;
use rattler_build_recipe::stage0::{Item, Script, SerializableMatchSpec, Value};
use rattler_conda_types::{ChannelUrl, Platform};
use thiserror::Error;

use crate::build_script::render_build_script;
use crate::config::{RosBackendConfig, RosBuildType, extract_distro_from_channels_list};

#[derive(Debug, Error, Diagnostic)]
pub enum PixiNativeError {
    #[error("could not determine ROS distro for pixi-native mode")]
    #[diagnostic(help(
        "Set `[package.build.config].distro = \"<name>\"`, add a `robostack-<distro>` channel, \
         or include at least one `ros-<distro>-*` package in your dependencies."
    ))]
    DistroUnresolved,

    #[error("dependencies reference multiple ROS distros: {distros:?}")]
    #[diagnostic(help(
        "A pixi-native package can only target one ROS distro at a time. \
         Remove or rename the conflicting `ros-<distro>-*` deps."
    ))]
    DistroConflict { distros: Vec<String> },
}

/// Resolve the ROS distro string for pixi-native mode.
///
/// Priority: explicit config > robostack-<distro> channel > inference from
/// `ros-<distro>-*` deps in the model.
pub fn resolve_distro(
    config: &RosBackendConfig,
    channels: &[ChannelUrl],
    model: &ProjectModel,
) -> Result<String, PixiNativeError> {
    if let Some(d) = &config.distro {
        return Ok(d.clone());
    }
    if let Some(d) = extract_distro_from_channels_list(channels) {
        return Ok(d);
    }
    infer_distro_from_model(model)
}

/// Walk every dep table on every target in the model. Collect the distinct
/// distro segments of any name matching `ros-<distro>-*`. One distinct value:
/// success. Zero: `DistroUnresolved`. More than one: `DistroConflict`.
fn infer_distro_from_model(model: &ProjectModel) -> Result<String, PixiNativeError> {
    let mut found: BTreeSet<String> = BTreeSet::new();

    if let Some(targets) = &model.targets {
        if let Some(default_target) = &targets.default_target {
            collect_distros_from_target(default_target, &mut found);
        }
        if let Some(platform_targets) = &targets.targets {
            for target in platform_targets.values() {
                collect_distros_from_target(target, &mut found);
            }
        }
    }

    match found.len() {
        0 => Err(PixiNativeError::DistroUnresolved),
        1 => Ok(found.into_iter().next().unwrap()),
        _ => Err(PixiNativeError::DistroConflict {
            distros: found.into_iter().collect(),
        }),
    }
}

fn collect_distros_from_target(target: &Target, found: &mut BTreeSet<String>) {
    let tables = [
        target.host_dependencies.as_ref(),
        target.build_dependencies.as_ref(),
        target.run_dependencies.as_ref(),
    ];
    for table in tables.into_iter().flatten() {
        for name in table.keys() {
            if let Some(distro) = name
                .as_str()
                .strip_prefix("ros-")
                .and_then(|rest| rest.split('-').next())
            {
                if !distro.is_empty() {
                    found.insert(distro.to_string());
                }
            }
        }
    }
}

/// Generate a recipe directly from the project model in pixi-native mode.
///
/// Starts from the framework's default model-derived recipe, then injects
/// ROS-specific build/host/run dependencies and renders the build script for
/// the configured `build_type`.
pub async fn generate(
    model: &ProjectModel,
    config: &RosBackendConfig,
    manifest_root: PathBuf,
    _host_platform: Platform,
    channels: Vec<ChannelUrl>,
) -> miette::Result<GeneratedRecipe> {
    use miette::IntoDiagnostic;

    let distro = resolve_distro(config, &channels, model).into_diagnostic()?;

    let build_type = config
        .build_type
        .ok_or_else(|| miette::miette!("build-type required in pixi-native mode"))?;

    if !config.extra_package_mappings.is_empty() {
        tracing::warn!(
            "extra-package-mappings is set but mode is pixi-native; the mappings will be ignored"
        );
    }

    // Start from the framework's default recipe-from-model. The framework
    // populates name, version, license, source, and the dep tables from the model.
    let mut generated = GeneratedRecipe::from_model(model.clone(), &mut DefaultMetadataProvider)
        .map_err(|e| miette::miette!("failed to derive recipe from model: {e:?}"))?;

    let mut build_items: Vec<Item<SerializableMatchSpec>> = Vec::new();
    let mut host_items: Vec<Item<SerializableMatchSpec>> = Vec::new();
    let mut run_items: Vec<Item<SerializableMatchSpec>> = Vec::new();

    // Standard build deps (linux subset of existing flow).
    for dep in [
        "cmake", "ninja", "python", "setuptools", "git", "git-lfs", "cpython", "patch", "make",
        "coreutils",
    ] {
        build_items.push(spec(dep));
    }
    build_items.push(template_value("${{ compiler('c') }}"));
    build_items.push(template_value("${{ compiler('cxx') }}"));

    // ament_cargo wants the rust toolchain plus the cargo-ament-build wrapper.
    if build_type == RosBuildType::AmentCargo {
        build_items.push(spec("rust"));
        build_items.push(spec(&format!("ros-{distro}-cargo-ament-build")));
    }

    // Standard host deps.
    for dep in ["python", "numpy", "pip", "pkg-config"] {
        host_items.push(spec(dep));
    }

    // ROS-specific injection: distro mutex (host + run) and ros-workspace (host + run).
    host_items.push(spec("ros2-distro-mutex"));
    run_items.push(spec("ros2-distro-mutex"));
    host_items.push(spec(&format!("ros-{distro}-ros-workspace")));
    run_items.push(spec(&format!("ros-{distro}-ros-workspace")));

    let req = &mut generated.recipe.requirements;
    req.build.extend(build_items);
    req.host.extend(host_items);
    req.run.extend(run_items);

    // Build script.
    let build_type_str = match build_type {
        RosBuildType::AmentCmake => "ament_cmake",
        RosBuildType::AmentPython => "ament_python",
        RosBuildType::AmentCargo => "ament_cargo",
    };
    let script_content = render_build_script(build_type_str, &distro, &manifest_root)
        .map_err(|e| miette::miette!("failed to render build script: {e}"))?;

    let mut script_env: IndexMap<String, Value<String>> = IndexMap::new();
    script_env.insert(
        "ROS_DISTRO".to_string(),
        Value::new_concrete(distro.clone(), None),
    );
    script_env.insert(
        "ROS_VERSION".to_string(),
        Value::new_concrete("2".to_string(), None),
    );
    if let Some(user_env) = &config.env {
        for (k, v) in user_env {
            script_env.insert(k.clone(), Value::new_concrete(v.clone(), None));
        }
    }

    generated.recipe.build.script = Script::from_content(script_content).with_env(script_env);

    // Add input globs the cache invalidator should watch.
    for glob in [
        "**/*.c",
        "**/*.cpp",
        "**/*.h",
        "**/*.hpp",
        "**/*.rs",
        "**/*.py",
        "**/*.pyx",
        "**/*.sh",
        "Cargo.toml",
        "Cargo.lock",
        "CMakeLists.txt",
        "Makefile",
        "MANIFEST.in",
        "setup.py",
        "setup.cfg",
        "pyproject.toml",
        "package.xml",
        "tests/**/*.py",
        "docs/**/*.rst",
        "docs/**/*.md",
        "launch/**/*.py",
        "config/*.yaml",
        "msg/**/*.msg",
        "srv/**/*.srv",
        "action/**/*.action",
    ] {
        generated.metadata_input_globs.insert(glob.to_string());
    }
    if let Some(extra) = &config.extra_input_globs {
        for g in extra {
            generated.metadata_input_globs.insert(g.clone());
        }
    }

    Ok(generated)
}

fn spec(name: &str) -> Item<SerializableMatchSpec> {
    Item::Value(Value::new_concrete(SerializableMatchSpec::from(name), None))
}

fn template_value(template_str: &str) -> Item<SerializableMatchSpec> {
    let tmpl =
        JinjaTemplate::new(template_str.to_string()).expect("valid jinja template");
    Item::Value(Value::new_template(tmpl, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_config() -> RosBackendConfig {
        RosBackendConfig::default()
    }

    fn model_with_deps(host: &[&str], run: &[&str]) -> ProjectModel {
        let host_obj = host
            .iter()
            .map(|n| (n.to_string(), serde_json::json!({"binary": {"version": "*"}})))
            .collect::<serde_json::Map<_, _>>();
        let run_obj = run
            .iter()
            .map(|n| (n.to_string(), serde_json::json!({"binary": {"version": "*"}})))
            .collect::<serde_json::Map<_, _>>();
        serde_json::from_value(serde_json::json!({
            "name": "test-pkg",
            "version": "0.1.0",
            "targets": {
                "defaultTarget": {
                    "hostDependencies": host_obj,
                    "buildDependencies": {},
                    "runDependencies": run_obj,
                },
                "targets": {}
            }
        }))
        .expect("ProjectModel fixture")
    }

    #[test]
    fn explicit_config_wins() {
        let mut cfg = empty_config();
        cfg.distro = Some("kilted".to_string());
        let model = model_with_deps(&["ros-jazzy-rclcpp"], &[]);
        let got = resolve_distro(&cfg, &[], &model).unwrap();
        assert_eq!(got, "kilted");
    }

    #[test]
    fn channel_used_when_no_explicit() {
        use url::Url;
        let cfg = empty_config();
        let channels = vec![ChannelUrl::from(
            Url::parse("https://prefix.dev/robostack-jazzy").unwrap(),
        )];
        let model = model_with_deps(&[], &[]);
        let got = resolve_distro(&cfg, &channels, &model).unwrap();
        assert_eq!(got, "jazzy");
    }

    #[test]
    fn dep_inference_when_neither_config_nor_channel() {
        let cfg = empty_config();
        let model = model_with_deps(&["ros-kilted-rclcpp", "ros-kilted-std-msgs"], &[]);
        let got = resolve_distro(&cfg, &[], &model).unwrap();
        assert_eq!(got, "kilted");
    }

    #[test]
    fn dep_inference_finds_run_deps_too() {
        let cfg = empty_config();
        let model = model_with_deps(&[], &["ros-kilted-rclcpp"]);
        let got = resolve_distro(&cfg, &[], &model).unwrap();
        assert_eq!(got, "kilted");
    }

    #[test]
    fn conflicting_distros_error() {
        let cfg = empty_config();
        let model = model_with_deps(&["ros-kilted-rclcpp", "ros-jazzy-rclcpp"], &[]);
        let err = resolve_distro(&cfg, &[], &model).unwrap_err();
        assert!(matches!(err, PixiNativeError::DistroConflict { .. }));
    }

    #[test]
    fn no_signal_errors() {
        let cfg = empty_config();
        let model = model_with_deps(&["numpy", "rich"], &[]);
        let err = resolve_distro(&cfg, &[], &model).unwrap_err();
        assert!(matches!(err, PixiNativeError::DistroUnresolved));
    }

    #[test]
    fn non_ros_prefixed_packages_ignored() {
        // Names starting with "ros-" but no second-segment-as-distro convention
        // (e.g. "ros-no") are still picked up — that's by design; the conda solver
        // will fail later if the distro segment is bogus.
        let cfg = empty_config();
        let model = model_with_deps(&["rosdep", "rosdistro"], &[]);
        let err = resolve_distro(&cfg, &[], &model).unwrap_err();
        // "rosdep" and "rosdistro" don't have "ros-" prefix so they're ignored.
        assert!(matches!(err, PixiNativeError::DistroUnresolved));
    }

    use crate::config::{RosBuildType, RosMode};
    use rattler_build_recipe::stage0::Item;
    use std::path::PathBuf;

    fn cfg_pixi_native(build_type: RosBuildType) -> RosBackendConfig {
        let mut cfg = RosBackendConfig::default();
        cfg.mode = Some(RosMode::PixiNative);
        cfg.build_type = Some(build_type);
        cfg.distro = Some("kilted".to_string());
        cfg
    }

    fn host_run_concrete(
        recipe: &rattler_build_recipe::stage0::SingleOutputRecipe,
    ) -> (Vec<String>, Vec<String>, Vec<String>) {
        fn names(
            items: &rattler_build_recipe::stage0::ConditionalList<
                rattler_build_recipe::stage0::SerializableMatchSpec,
            >,
        ) -> Vec<String> {
            items
                .iter()
                .filter_map(|i| match i {
                    Item::Value(v) => v.as_concrete().map(|s| s.to_string()),
                    _ => None,
                })
                .collect()
        }
        (
            names(&recipe.requirements.build),
            names(&recipe.requirements.host),
            names(&recipe.requirements.run),
        )
    }

    #[tokio::test]
    async fn generate_ament_cmake_injects_workspace_and_mutex() {
        let cfg = cfg_pixi_native(RosBuildType::AmentCmake);
        let model = model_with_deps(&["ros-kilted-rclcpp"], &["ros-kilted-rclcpp"]);
        let recipe = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        .unwrap();

        let (build, host, run) = host_run_concrete(&recipe.recipe);
        assert!(host.iter().any(|s| s == "ros2-distro-mutex"));
        assert!(run.iter().any(|s| s == "ros2-distro-mutex"));
        assert!(host.iter().any(|s| s == "ros-kilted-ros-workspace"));
        assert!(run.iter().any(|s| s == "ros-kilted-ros-workspace"));
        assert!(build.iter().any(|s| s == "cmake"));
        assert!(build.iter().any(|s| s == "ninja"));
    }

    #[tokio::test]
    async fn generate_ament_cargo_injects_rust_and_cargo_ament_build() {
        let cfg = cfg_pixi_native(RosBuildType::AmentCargo);
        let model = model_with_deps(&["ros-kilted-rclrs"], &["ros-kilted-rclrs"]);
        let recipe = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        .unwrap();

        let (build, _, _) = host_run_concrete(&recipe.recipe);
        assert!(build.iter().any(|s| s == "rust"));
        assert!(build.iter().any(|s| s == "ros-kilted-cargo-ament-build"));
    }

    #[tokio::test]
    async fn generate_ament_python_does_not_inject_rust() {
        let cfg = cfg_pixi_native(RosBuildType::AmentPython);
        let model = model_with_deps(&["ros-kilted-rclpy"], &["ros-kilted-rclpy"]);
        let recipe = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        .unwrap();
        let (build, _, _) = host_run_concrete(&recipe.recipe);
        assert!(!build.iter().any(|s| s == "rust"));
        assert!(!build.iter().any(|s| s == "ros-kilted-cargo-ament-build"));
    }

    #[tokio::test]
    async fn generate_missing_build_type_errors() {
        let mut cfg = RosBackendConfig::default();
        cfg.mode = Some(RosMode::PixiNative);
        cfg.distro = Some("kilted".to_string());
        let model = model_with_deps(&["ros-kilted-rclcpp"], &[]);
        let result = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await;
        let err = match result {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => e,
        };
        assert!(format!("{:?}", err).contains("build-type"));
    }
}
