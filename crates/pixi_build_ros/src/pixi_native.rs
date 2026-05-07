//! Pixi-native code path: generate a recipe directly from `pixi.toml`,
//! without any `package.xml`, rosdep mapping, or rosdistro fetch.
//!
//! Mode is selected by `RosMode::PixiNative` (or auto-detected when no
//! `package.xml` is present alongside the manifest).

use std::collections::BTreeSet;

use miette::Diagnostic;
use pixi_build_types::{ProjectModel, Target};
use rattler_conda_types::ChannelUrl;
use thiserror::Error;

use crate::config::{RosBackendConfig, extract_distro_from_channels_list};

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
}
