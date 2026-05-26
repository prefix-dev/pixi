//! Pixi-native code path: generate a recipe directly from `pixi.toml`,
//! without any `package.xml`, rosdep mapping, or rosdistro fetch.
//!
//! Mode is selected by `RosMode::PixiNative` (or auto-detected when no
//! `package.xml` is present alongside the manifest).

use std::collections::BTreeSet;
use std::path::PathBuf;

use indexmap::IndexMap;
use miette::Diagnostic;
use pixi_build_backend::generated_recipe::{DefaultMetadataProvider, GeneratedRecipe};
use pixi_build_types::{ProjectModel, Target};
use rattler_build_jinja::JinjaTemplate;
use rattler_build_recipe::stage0::{Item, Script, SerializableMatchSpec, Value};
use rattler_conda_types::{ChannelUrl, NoArchType, Platform};
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

    #[error("`build-type` is required when `mode = pixi-native`")]
    #[diagnostic(help(
        "Set `[package.build.config].build-type` to one of `ament_cmake`, `ament_python`, or `ament_cargo`."
    ))]
    BuildTypeRequired,

    #[error("invalid ROS distro name: '{distro}'")]
    #[diagnostic(help(
        "ROS distro names must contain only letters, digits, `-`, `_`, or `.` characters."
    ))]
    InvalidDistroName { distro: String },
}

/// Validate a ROS distro string before it's interpolated into conda package
/// names. Conda package names accept `[0-9a-zA-Z\-_.]`; anything else would
/// later panic inside `SerializableMatchSpec::from`.
fn validate_distro(distro: &str) -> Result<(), PixiNativeError> {
    if distro.is_empty()
        || !distro
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err(PixiNativeError::InvalidDistroName {
            distro: distro.to_string(),
        });
    }
    Ok(())
}

/// Resolve the ROS distro string for pixi-native mode.
///
/// Priority: explicit config > `robostack-<distro>` channel > inference from
/// `ros-<distro>-*` deps in the model.
pub fn resolve_distro(
    config: &RosBackendConfig,
    channels: &[ChannelUrl],
    model: &ProjectModel,
) -> Result<String, PixiNativeError> {
    let distro = if let Some(d) = &config.distro {
        d.clone()
    } else if let Some(d) = extract_distro_from_channels_list(channels) {
        d
    } else {
        infer_distro_from_model(model)?
    };
    validate_distro(&distro)?;
    Ok(distro)
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
                .filter(|d| !d.is_empty())
            {
                found.insert(distro.to_string());
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
    let distro = resolve_distro(config, &channels, model)?;

    let build_type = config
        .build_type
        .ok_or(PixiNativeError::BuildTypeRequired)?;

    // Default ament_python packages to noarch unless the user explicitly opts out.
    // Other build types remain platform-specific unless the user explicitly opts in.
    let is_noarch = match (config.noarch, build_type) {
        (Some(v), _) => v,
        (None, RosBuildType::AmentPython) => true,
        (None, _) => false,
    };

    if !config.extra_package_mappings.is_empty() {
        tracing::warn!(
            "extra-package-mappings is set but mode is pixi-native; the mappings will be ignored"
        );
    }

    // Start from the framework's default recipe-from-model. The framework
    // populates name, version, license, source, and the dep tables from the model.
    //
    // If the user opted into `prefix-with-distro`, rewrite the model name to
    // `ros-<distro>-<name>` before handing it to the framework so that both
    // the recipe `package.name` and any downstream uses see the prefixed
    // form. Dependency names in the model are NOT rewritten — only the
    // produced package name.
    let mut model_for_recipe = model.clone();
    if config.prefix_with_distro.unwrap_or(false) {
        if let Some(name) = &model_for_recipe.name {
            if !name.starts_with(&format!("ros-{distro}-")) {
                model_for_recipe.name = Some(format!("ros-{distro}-{name}"));
            }
        }
    }
    let mut generated = GeneratedRecipe::from_model(model_for_recipe, &mut DefaultMetadataProvider)
        .map_err(|e| miette::miette!("failed to derive recipe from model: {e:?}"))?;

    let mut build_items: Vec<Item<SerializableMatchSpec>> = Vec::new();
    let mut host_items: Vec<Item<SerializableMatchSpec>> = Vec::new();
    let mut run_items: Vec<Item<SerializableMatchSpec>> = Vec::new();

    // Standard build deps (linux subset of existing flow).
    // On noarch, the build runs once on the build-platform runner and produces
    // an arch-independent artifact, so the C/C++ toolchain and OS-specific
    // shims are irrelevant — emitting them forces a compiler-variant axis
    // through the recipe for no reason.
    let mut common_build_deps: Vec<&str> = vec![
        "cmake",
        "ninja",
        "python",
        "setuptools",
        "git",
        "git-lfs",
        "cpython",
    ];
    if !is_noarch {
        common_build_deps.extend(["patch", "make", "coreutils"]);
    }
    for dep in common_build_deps {
        build_items.push(spec(dep));
    }
    if !is_noarch {
        build_items.push(template_value("${{ compiler('c') }}"));
        build_items.push(template_value("${{ compiler('cxx') }}"));
    }

    // ament_cargo wants the rust toolchain plus the cargo-ament-build wrapper.
    // The C-side libs (rcl + msg packages) are also injected here because the
    // crates.io `rclrs` crate that downstream ament_cargo packages pull in
    // unconditionally vendors Rust bindings for the standard interface
    // packages and emits `#[link(name = "<pkg>__rosidl_(typesupport|
    // generator)_c")]` on the bound externs. Its build.rs additionally emits
    // `cargo:rustc-link-lib=dylib={rcl,rcl_action,rcl_yaml_param_parser,
    // rcutils,rmw,rmw_implementation}`. The libs must be host-visible at
    // build time and run-visible to the produced binary. They're injected
    // unconditionally for `ament_cargo` because the rclrs vendor module is
    // compiled unconditionally — opting out for the rare Rust-without-rclrs
    // case would be a future flag.
    if build_type == RosBuildType::AmentCargo {
        build_items.push(spec("rust"));
        build_items.push(spec(&format!("ros-{distro}-cargo-ament-build")));
        for pkg in [
            "rcl",
            "rcl-action",
            "action-msgs",
            "builtin-interfaces",
            "example-interfaces",
            "rcl-interfaces",
            "rosgraph-msgs",
            "test-msgs",
            "unique-identifier-msgs",
        ] {
            host_items.push(spec(&format!("ros-{distro}-{pkg}")));
            run_items.push(spec(&format!("ros-{distro}-{pkg}")));
        }
    }

    // ament_idl auto-injects every rosidl generator we know about plus the
    // runtimes their generated artifacts depend on. Generators register
    // themselves as ament extensions at install time, so simply having them
    // in the host env is enough to invoke them — no CMakeLists.txt edits
    // needed by the consumer. Rule of thumb: if you want consumers to get a
    // language binding for free, add its generator here AND its runtime to
    // run-deps below.
    if build_type == RosBuildType::AmentIdl {
        for generator in [
            "rosidl-default-generators",   // c, cpp, py
            "rosidl-generator-pydantic",
            "rosidl-generator-mypy",
            "rosidl-generator-rs",
        ] {
            host_items.push(spec(&format!("ros-{distro}-{generator}")));
        }
        for runtime in ["rosidl-default-runtime", "rosidl-runtime-rs"] {
            host_items.push(spec(&format!("ros-{distro}-{runtime}")));
            run_items.push(spec(&format!("ros-{distro}-{runtime}")));
        }
    }

    // Standard host deps.
    for dep in ["python", "numpy", "pip", "pkg-config"] {
        host_items.push(spec(dep));
    }

    // ROS-specific injection: distro mutex (host + run) and ros-workspace (host + run).
    host_items.push(spec("ros2-distro-mutex 0.15.*"));
    run_items.push(spec("ros2-distro-mutex 0.15.*"));
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
        RosBuildType::AmentIdl => "ament_idl",
    };
    // All ament_* build types need a synthesized package.xml at build time:
    // ament_package() / setup.py / cargo-ament-build all read or install it.
    // Plain cmake/catkin builds ignore this argument.
    let synth_xml = Some(synthesize_package_xml(model, build_type));
    let script_content =
        render_build_script(build_type_str, &distro, &manifest_root, synth_xml.as_deref())
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

    generated.recipe.build.script = Script::from_content(script_content)
        .with_env(script_env)
        .with_secrets(model.secrets.iter().cloned().collect());

    if let Some(n) = config.build_number {
        generated.recipe.build.number = Some(Value::new_concrete(n, None));
    }

    if is_noarch {
        generated.recipe.build.noarch =
            Some(Value::new_concrete(NoArchType::python(), None));
    }

    // Add input globs the cache invalidator should watch. Pixi-native mode
    // doesn't distinguish editable installs, so include the python globs too.
    for glob in crate::globs::ROS_SOURCE_GLOBS
        .iter()
        .chain(crate::globs::ROS_PYTHON_SOURCE_GLOBS.iter())
    {
        generated.metadata_input_globs.insert((*glob).to_string());
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

/// Synthesize a minimal package.xml for a pixi-native package.
///
/// Build/run dep declarations are intentionally omitted: the consumer's
/// CMakeLists.txt / setup.py / Cargo.toml has explicit references to every
/// dep that matters at build time, and conda manages runtime deps
/// independently. Translating conda package names back to rosdep keys would
/// require a mapping table we don't want to own.
///
/// What we DO emit:
///   - metadata (name, version, description, license, maintainer)
///   - one `<buildtool_depend>` matching the ament flavor
///   - for ament_idl, additionally `rosidl_default_generators` as a
///     `<buildtool_depend>` and the
///     `<member_of_group>rosidl_interface_packages</member_of_group>`
///     declaration that rosidl_cmake hard-requires
///   - the matching `<export><build_type>` tag
///
/// Maintainer is required by the package format. We parse the first author
/// in the model; if absent or unparseable, fall back to a placeholder.
fn synthesize_package_xml(model: &ProjectModel, build_type: RosBuildType) -> String {
    // ROS package names must be snake_case; pixi.toml often uses kebab-case.
    let name = model
        .name
        .as_deref()
        .unwrap_or("unnamed")
        .replace('-', "_");
    let name = name.as_str();
    let version = model
        .version
        .as_ref()
        .map(|v| v.to_string())
        .unwrap_or_else(|| "0.0.0".to_string());
    let description = model
        .description
        .as_deref()
        .unwrap_or("Generated by pixi-build-ros from pixi.toml");
    let license = model.license.as_deref().unwrap_or("UNLICENSED");
    let (maintainer_name, maintainer_email) = model
        .authors
        .as_ref()
        .and_then(|a| a.first())
        .map(|a| parse_author(a))
        .unwrap_or_else(|| {
            (
                "pixi-build-ros".to_string(),
                "noreply@example.com".to_string(),
            )
        });

    // <build_type> in the export section follows ROS conventions; ament_idl
    // packages are built with ament_cmake under the hood, so they declare
    // ament_cmake here.
    let (buildtool, export_build_type) = match build_type {
        RosBuildType::AmentCmake => ("ament_cmake", "ament_cmake"),
        RosBuildType::AmentPython => ("ament_python", "ament_python"),
        RosBuildType::AmentCargo => ("ament_cargo", "ament_cargo"),
        RosBuildType::AmentIdl => ("ament_cmake", "ament_cmake"),
    };

    let mut buildtool_block = format!("  <buildtool_depend>{buildtool}</buildtool_depend>\n");
    let mut group_block = String::new();
    if build_type == RosBuildType::AmentIdl {
        buildtool_block
            .push_str("  <buildtool_depend>rosidl_default_generators</buildtool_depend>\n");
        group_block
            .push_str("  <member_of_group>rosidl_interface_packages</member_of_group>\n");
    }

    format!(
        r#"<?xml version="1.0"?>
<?xml-model href="http://download.ros.org/schema/package_format3.xsd" schematypens="http://www.w3.org/2001/XMLSchema"?>
<package format="3">
  <name>{name}</name>
  <version>{version}</version>
  <description>{description}</description>
  <maintainer email="{email}">{maintainer}</maintainer>
  <license>{license}</license>

{buildtool_block}{group_block}
  <export>
    <build_type>{export_build_type}</build_type>
  </export>
</package>
"#,
        name = xml_escape(name),
        version = xml_escape(&version),
        description = xml_escape(description),
        license = xml_escape(license),
        email = xml_escape(&maintainer_email),
        maintainer = xml_escape(&maintainer_name),
    )
}

/// Parse `"Name <email@example.com>"` into `(name, email)`. Falls back to the
/// whole string as name with a placeholder email if the angle-bracket form
/// isn't present.
fn parse_author(author: &str) -> (String, String) {
    if let Some(open) = author.rfind('<') {
        if let Some(close) = author[open..].find('>') {
            let name = author[..open].trim().trim_end_matches(',').trim().to_string();
            let email = author[open + 1..open + close].trim().to_string();
            if !name.is_empty() && !email.is_empty() {
                return (name, email);
            }
        }
    }
    (author.trim().to_string(), "noreply@example.com".to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn template_value(template_str: &str) -> Item<SerializableMatchSpec> {
    let tmpl = JinjaTemplate::new(template_str.to_string()).expect("valid jinja template");
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
            .map(|n| {
                (
                    n.to_string(),
                    serde_json::json!({"binary": {"version": "*"}}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let run_obj = run
            .iter()
            .map(|n| {
                (
                    n.to_string(),
                    serde_json::json!({"binary": {"version": "*"}}),
                )
            })
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
    use rattler_conda_types::NoArchType;
    use std::path::PathBuf;

    fn recipe_noarch(
        recipe: &rattler_build_recipe::stage0::SingleOutputRecipe,
    ) -> Option<rattler_conda_types::NoArchType> {
        recipe
            .build
            .noarch
            .as_ref()
            .and_then(|v| v.as_concrete().copied())
    }

    fn cfg_pixi_native(build_type: RosBuildType) -> RosBackendConfig {
        RosBackendConfig {
            mode: Some(RosMode::PixiNative),
            build_type: Some(build_type),
            distro: Some("kilted".to_string()),
            ..Default::default()
        }
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
        assert!(host.iter().any(|s| s == "ros2-distro-mutex 0.15.*"));
        assert!(run.iter().any(|s| s == "ros2-distro-mutex 0.15.*"));
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

        let (build, host, run) = host_run_concrete(&recipe.recipe);
        assert!(build.iter().any(|s| s == "rust"));
        assert!(build.iter().any(|s| s == "ros-kilted-cargo-ament-build"));
        // The C-side libs that crates.io `rclrs` links against must be in host
        // (visible to the cargo build) and run (visible to the produced binary).
        for pkg in [
            "ros-kilted-rcl",
            "ros-kilted-rcl-action",
            "ros-kilted-action-msgs",
            "ros-kilted-builtin-interfaces",
            "ros-kilted-example-interfaces",
            "ros-kilted-rcl-interfaces",
            "ros-kilted-rosgraph-msgs",
            "ros-kilted-test-msgs",
            "ros-kilted-unique-identifier-msgs",
        ] {
            assert!(host.iter().any(|s| s == pkg), "missing in host: {pkg}");
            assert!(run.iter().any(|s| s == pkg), "missing in run: {pkg}");
        }
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
        let (build, host, run) = host_run_concrete(&recipe.recipe);
        assert!(!build.iter().any(|s| s == "rust"));
        assert!(!build.iter().any(|s| s == "ros-kilted-cargo-ament-build"));
        // The rclrs C-side host/run injections are ament_cargo-only.
        assert!(!host.iter().any(|s| s == "ros-kilted-rcl"));
        assert!(!run.iter().any(|s| s == "ros-kilted-rcl"));
    }

    #[tokio::test]
    async fn generate_ament_python_defaults_to_noarch() {
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

        assert_eq!(
            recipe_noarch(&recipe.recipe),
            Some(NoArchType::python()),
            "ament_python with no opt-out must default to noarch: python",
        );

        let (build, _host, _run) = host_run_concrete(&recipe.recipe);
        // Concrete build deps don't include compiler templates (templates aren't
        // emitted by host_run_concrete's `as_concrete` filter), so assert by
        // serializing the full conditional list and string-searching the template
        // form pixi-build-ros injects.
        let build_yaml = serde_yaml::to_string(&recipe.recipe.requirements.build).unwrap();
        assert!(
            !build_yaml.contains("compiler('c')"),
            "noarch ament_python build deps must not include compiler('c'):\n{build_yaml}"
        );
        assert!(
            !build_yaml.contains("compiler('cxx')"),
            "noarch ament_python build deps must not include compiler('cxx'):\n{build_yaml}"
        );
        // Sanity: ament_python should still get python/setuptools etc.
        assert!(build.iter().any(|s| s == "python"));
        assert!(build.iter().any(|s| s == "setuptools"));
    }

    #[tokio::test]
    async fn generate_ament_python_noarch_false_opts_out() {
        let mut cfg = cfg_pixi_native(RosBuildType::AmentPython);
        cfg.noarch = Some(false);
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

        assert_eq!(
            recipe_noarch(&recipe.recipe),
            None,
            "explicit noarch=false must override the ament_python default",
        );
        let build_yaml = serde_yaml::to_string(&recipe.recipe.requirements.build).unwrap();
        assert!(
            build_yaml.contains("compiler('c')"),
            "non-noarch ament_python must still inject compiler('c'):\n{build_yaml}"
        );
    }

    #[tokio::test]
    async fn generate_ament_cmake_is_not_noarch_by_default() {
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

        assert_eq!(
            recipe_noarch(&recipe.recipe),
            None,
            "ament_cmake must remain platform-specific",
        );
        let build_yaml = serde_yaml::to_string(&recipe.recipe.requirements.build).unwrap();
        assert!(
            build_yaml.contains("compiler('c')"),
            "ament_cmake must still inject compiler('c'):\n{build_yaml}"
        );
    }

    #[tokio::test]
    async fn package_name_unprefixed_by_default() {
        let cfg = cfg_pixi_native(RosBuildType::AmentCmake);
        let model = model_with_deps(&["ros-kilted-rclcpp"], &[]);
        let recipe = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        .unwrap();
        let name = recipe
            .recipe
            .package
            .name
            .as_concrete()
            .expect("concrete name")
            .to_string();
        assert_eq!(name, "test-pkg");
    }

    #[tokio::test]
    async fn package_name_prefixed_when_flag_set() {
        let mut cfg = cfg_pixi_native(RosBuildType::AmentCmake);
        cfg.prefix_with_distro = Some(true);
        let model = model_with_deps(&["ros-kilted-rclcpp"], &[]);
        let recipe = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        .unwrap();
        let name = recipe
            .recipe
            .package
            .name
            .as_concrete()
            .expect("concrete name")
            .to_string();
        assert_eq!(name, "ros-kilted-test-pkg");
    }

    #[tokio::test]
    async fn package_name_not_double_prefixed() {
        let mut cfg = cfg_pixi_native(RosBuildType::AmentCmake);
        cfg.prefix_with_distro = Some(true);
        // model name already has the prefix; ensure we don't add another.
        let mut model = model_with_deps(&["ros-kilted-rclcpp"], &[]);
        model.name = Some("ros-kilted-test-pkg".to_string());
        let recipe = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        .unwrap();
        let name = recipe
            .recipe
            .package
            .name
            .as_concrete()
            .expect("concrete name")
            .to_string();
        assert_eq!(name, "ros-kilted-test-pkg");
    }

    #[tokio::test]
    async fn build_number_applied_from_config() {
        let mut cfg = cfg_pixi_native(RosBuildType::AmentCmake);
        cfg.build_number = Some(7);
        let model = model_with_deps(&["ros-kilted-rclcpp"], &[]);
        let recipe = generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        .unwrap();
        assert_eq!(
            recipe
                .recipe
                .build
                .number
                .as_ref()
                .and_then(|v| v.as_concrete().copied()),
            Some(7)
        );
    }

    #[tokio::test]
    async fn generate_missing_build_type_errors() {
        let cfg = RosBackendConfig {
            mode: Some(RosMode::PixiNative),
            distro: Some("kilted".to_string()),
            ..Default::default()
        };
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
        let typed = err
            .downcast_ref::<PixiNativeError>()
            .expect("expected a PixiNativeError");
        assert!(matches!(typed, PixiNativeError::BuildTypeRequired));
    }

    #[tokio::test]
    async fn generate_invalid_distro_name_errors() {
        let cfg = RosBackendConfig {
            mode: Some(RosMode::PixiNative),
            build_type: Some(RosBuildType::AmentCmake),
            distro: Some("kilted hat".to_string()), // space invalid
            ..Default::default()
        };
        let model = model_with_deps(&[], &[]);
        let err = match generate(
            &model,
            &cfg,
            PathBuf::from("/tmp/fake"),
            rattler_conda_types::Platform::Linux64,
            vec![],
        )
        .await
        {
            Ok(_) => panic!("expected error, got Ok"),
            Err(e) => e,
        };
        let typed = err
            .downcast_ref::<PixiNativeError>()
            .expect("expected a PixiNativeError");
        assert!(matches!(typed, PixiNativeError::InvalidDistroName { .. }));
    }

    #[tokio::test]
    async fn pixi_native_ament_cmake_recipe() {
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

        insta::assert_yaml_snapshot!(recipe.recipe, {
            ".source[0].path" => "[path]",
            ".build.script" => "[script]",
        });
    }
}
