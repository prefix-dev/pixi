/// This file contains the test model, which is a minimal example of a ProjectModel
/// that can be used to create a ProjectModel from a JSON fixture file.
use pixi_build_types::{
    BinaryPackageSpec as PbtBinaryPackageSpec, PackageSpec as PbtPackageSpec, PathSpec,
    ProjectModel, Target as PbtTarget, TargetSelector as PbtTargetSelector, Targets as PbtTargets,
};

use rattler_conda_types::{ParseStrictness, Version, VersionSpec};

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestProjectModel {
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    pub license_file: Option<String>,
    pub readme: Option<String>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub documentation: Option<String>,
    pub targets: Targets,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Targets {
    pub default_target: Target,
    pub targets: HashMap<TargetSelector, Target>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub host_dependencies: HashMap<String, PackageSpec>,
    pub build_dependencies: HashMap<String, PackageSpec>,
    pub run_dependencies: HashMap<String, PackageSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageSpec {
    Binary(BinaryPackageSpec),
    Source(SourcePackageSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinaryPackageSpec {
    pub binary: BinarySpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BinarySpec {
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourcePackageSpec {
    pub source: SourceSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSpec {
    pub version: Option<String>,
    pub path: Option<String>,
    pub git: Option<String>,
    pub url: Option<String>,
}

/// Represents a target selector. Currently, we only support explicit platform
/// selection.
#[derive(Debug, Clone, Deserialize, Serialize, Hash, Eq, PartialEq)]
pub enum TargetSelector {
    // Platform specific configuration
    Unix,
    Linux,
    Win,
    MacOs,
    Platform(String),
}

/// Helper function to load a test ProjectModel from a JSON fixture file
pub(crate) fn load_project_model_from_json(filename: &str) -> TestProjectModel {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(filename);

    let json_content = std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read JSON fixture '{filename}': {e}"));

    serde_json::from_str(&json_content)
        .unwrap_or_else(|e| panic!("Failed to parse JSON fixture '{filename}': {e}"))
}

/// Converts a TestProjectModel into a ProjectModel
pub(crate) fn convert_test_model_to_project_model_v1(test_model: TestProjectModel) -> ProjectModel {
    use std::str::FromStr;

    // Convert the targets
    let targets_v1 = PbtTargets {
        default_target: Some(convert_target_to_v1(&test_model.targets.default_target)),
        targets: Some(
            test_model
                .targets
                .targets
                .into_iter()
                .map(|(selector, target)| {
                    (
                        convert_target_selector_to_v1(selector),
                        convert_target_to_v1(&target),
                    )
                })
                .collect(),
        ),
    };

    ProjectModel {
        name: Some(test_model.name),
        version: Some(Version::from_str(&test_model.version).unwrap()),
        description: test_model.description,
        authors: test_model.authors,
        license: test_model.license,
        license_file: test_model.license_file.map(PathBuf::from),
        readme: test_model.readme.map(PathBuf::from),
        homepage: test_model.homepage.and_then(|h| url::Url::parse(&h).ok()),
        repository: test_model.repository.and_then(|r| url::Url::parse(&r).ok()),
        documentation: test_model
            .documentation
            .and_then(|d| url::Url::parse(&d).ok()),
        targets: Some(targets_v1),
        build_number: None,
        build_string: None,
    }
}

/// Converts a test Target to Target
fn convert_target_to_v1(target: &Target) -> PbtTarget {
    PbtTarget {
        build_dependencies: Some(
            target
                .build_dependencies
                .iter()
                .map(|(name, spec)| (name.clone(), convert_package_spec_to_v1(spec)))
                .collect(),
        ),
        host_dependencies: Some(
            target
                .host_dependencies
                .iter()
                .map(|(name, spec)| (name.clone(), convert_package_spec_to_v1(spec)))
                .collect(),
        ),
        run_dependencies: Some(
            target
                .run_dependencies
                .iter()
                .map(|(name, spec)| (name.clone(), convert_package_spec_to_v1(spec)))
                .collect(),
        ),
    }
}

/// Converts a test TargetSelector to TargetSelector
fn convert_target_selector_to_v1(selector: TargetSelector) -> PbtTargetSelector {
    match selector {
        TargetSelector::Unix => PbtTargetSelector::Unix,
        TargetSelector::Linux => PbtTargetSelector::Linux,
        TargetSelector::Win => PbtTargetSelector::Win,
        TargetSelector::MacOs => PbtTargetSelector::MacOs,
        TargetSelector::Platform(p) => PbtTargetSelector::Platform(p),
    }
}

/// Converts a test PackageSpec to PackageSpec
fn convert_package_spec_to_v1(spec: &PackageSpec) -> PbtPackageSpec {
    match spec {
        PackageSpec::Binary(binary_spec) => {
            let version_spec =
                VersionSpec::from_str(&binary_spec.binary.version, ParseStrictness::Lenient)
                    .unwrap_or(VersionSpec::Any);

            PbtPackageSpec::Binary(PbtBinaryPackageSpec {
                version: Some(version_spec),
                build: None,
                build_number: None,
                file_name: None,
                channel: None,
                subdir: None,
                md5: None,
                sha256: None,
                url: None,
                license: None,
            })
        }
        PackageSpec::Source(source_spec) => {
            let inside_source = source_spec.source.clone();
            if let Some(path) = inside_source.path {
                PbtPackageSpec::Source(PathSpec { path }.into())
            } else {
                unimplemented!("Only path source specs are supported for now");
            }
        }
    }
}
