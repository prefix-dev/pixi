//! Conversion functions from `pixi_spec` types to `pixi_build_types` types.
//! these are used to convert the `pixi_spec` types to the `pixi_build_types`
//! types we want to keep the conversion here, as we do not want
//! `pixi_build_types` to depend on `pixi_spec`
//!
//! This will mostly be boilerplate conversions but some of these are a bit more
//! interesting

use ordermap::OrderMap;
// Namespace to pbt, *please use* exclusively so we do not get confused between the two
// different types
use pixi_build_types::{self as pbt};

use pixi_manifest::{PackageManifest, PackageTarget, TargetSelector, Targets};
use pixi_spec::{GitReference, PixiSpec, SourceSpec, SpecConversionError};
use rattler_conda_types::{ChannelConfig, NamelessMatchSpec, PackageName};

/// Conversion from a `PixiSpec` to a `pbt::PixiSpecV1`.
fn to_pixi_spec_v1(
    spec: &PixiSpec,
    channel_config: &ChannelConfig,
) -> Result<pbt::PackageSpec, SpecConversionError> {
    // Convert into source or binary
    let source_or_binary = spec.clone().into_source_or_binary();
    // Convert into correct type for pixi
    let pbt_spec = match source_or_binary {
        itertools::Either::Left(source) => {
            let SourceSpec {
                location,
                version,
                build,
                build_number,
                extras: None,
                subdir,
                namespace: None,
                license,
                condition: None,
            } = source
            else {
                unimplemented!(
                    "a particular field is not implemented in the pixi to pbt conversion"
                );
            };
            let location = match location {
                pixi_spec::SourceLocationSpec::Url(url_source_spec) => {
                    let pixi_spec::UrlSourceSpec {
                        url,
                        md5,
                        sha256,
                        subdirectory,
                    } = url_source_spec;
                    pbt::SourcePackageLocationSpec::Url(pbt::UrlSpec {
                        url,
                        md5,
                        sha256,
                        subdirectory: subdirectory.to_option_string(),
                    })
                }
                pixi_spec::SourceLocationSpec::Git(git_spec) => {
                    let pixi_spec::GitSpec {
                        git,
                        rev,
                        subdirectory,
                    } = git_spec;
                    pbt::SourcePackageLocationSpec::Git(pbt::GitSpec {
                        git,
                        rev: rev.map(|r| match r {
                            GitReference::Branch(b) => pbt::GitReference::Branch(b),
                            GitReference::Tag(t) => pbt::GitReference::Tag(t),
                            GitReference::Rev(rev) => pbt::GitReference::Rev(rev),
                            GitReference::DefaultBranch => pbt::GitReference::DefaultBranch,
                        }),
                        subdirectory: subdirectory.to_option_string(),
                    })
                }
                pixi_spec::SourceLocationSpec::Path(path_source_spec) => {
                    pbt::SourcePackageLocationSpec::Path(pbt::PathSpec {
                        path: path_source_spec.path.to_string(),
                    })
                }
            };
            pbt::PackageSpec::Source(pbt::SourcePackageSpec {
                location,
                version,
                build,
                build_number,
                subdir,
                license,
            })
        }
        itertools::Either::Right(binary) => {
            let NamelessMatchSpec {
                version,
                build,
                build_number,
                file_name,
                channel,
                subdir,
                md5,
                sha256,
                url,
                license,
                // These are currently explicitly ignored in the conversion
                namespace: _,
                extras: _,
                condition: _,
                track_features: _,
                flags: _,
                license_family: _,
            } = binary.try_into_nameless_match_spec(channel_config)?;
            pbt::PackageSpec::Binary(pbt::BinaryPackageSpec {
                version,
                build,
                build_number,
                file_name,
                channel: channel.map(|c| c.base_url.url().clone().into()),
                subdir,
                md5,
                sha256,
                url,
                license,
            })
        }
    };
    Ok(pbt_spec)
}

/// Converts an iterator of `PackageName` and `PixiSpec` to a `IndexMap<String,
/// pbt::PixiSpecV1>`.
fn to_pbt_dependencies<'a>(
    iter: impl Iterator<Item = (&'a PackageName, &'a PixiSpec)>,
    channel_config: &ChannelConfig,
) -> Result<OrderMap<pbt::SourcePackageName, pbt::PackageSpec>, SpecConversionError> {
    iter.map(|(name, spec)| {
        let converted = to_pixi_spec_v1(spec, channel_config)?;
        Ok((pbt::SourcePackageName::from(name.clone()), converted))
    })
    .collect()
}

/// Converts a [`PackageTarget`] to a [`pbt::Target`].
fn to_target_v1(
    target: &PackageTarget,
    channel_config: &ChannelConfig,
) -> Result<pbt::Target, SpecConversionError> {
    // Difference for us is that [`pbt::TargetV1`] has split the host, run and build
    // dependencies into separate fields, so we need to split them up here
    Ok(pbt::Target {
        host_dependencies: Some(
            target
                .host_dependencies()
                .map(|deps| to_pbt_dependencies(deps.iter_specs(), channel_config))
                .transpose()?
                .unwrap_or_default(),
        ),
        build_dependencies: Some(
            target
                .build_dependencies()
                .map(|deps| to_pbt_dependencies(deps.iter_specs(), channel_config))
                .transpose()?
                .unwrap_or_default(),
        ),
        run_dependencies: Some(
            target
                .run_dependencies()
                .map(|deps| to_pbt_dependencies(deps.iter_specs(), channel_config))
                .transpose()?
                .unwrap_or_default(),
        ),
        run_constraints: Some(
            target
                .run_constraints()
                .map(|deps| to_pbt_dependencies(deps.iter_specs(), channel_config))
                .transpose()?
                .unwrap_or_default(),
        ),
    })
}

pub fn to_target_selector_v1(selector: &TargetSelector) -> pbt::TargetSelector {
    match selector {
        TargetSelector::Platform(platform) => pbt::TargetSelector::Platform(platform.to_string()),
        TargetSelector::Unix => pbt::TargetSelector::Unix,
        TargetSelector::Linux => pbt::TargetSelector::Linux,
        TargetSelector::Win => pbt::TargetSelector::Win,
        TargetSelector::MacOs => pbt::TargetSelector::MacOs,
    }
}

fn to_targets_v1(
    targets: &Targets<PackageTarget>,
    channel_config: &ChannelConfig,
) -> Result<pbt::Targets, SpecConversionError> {
    let selected_targets = targets
        .iter()
        .filter_map(|(k, v)| {
            v.map(|selector| {
                to_target_v1(k, channel_config)
                    .map(|target| (to_target_selector_v1(selector), target))
            })
        })
        .collect::<Result<OrderMap<pbt::TargetSelector, pbt::Target>, _>>()?;

    Ok(pbt::Targets {
        default_target: Some(to_target_v1(targets.default(), channel_config)?),
        targets: Some(selected_targets),
    })
}

/// Converts a [`PackageManifest`] to a [`pbt::ProjectModel`].
pub fn to_project_model_v1(
    manifest: &PackageManifest,
    channel_config: &ChannelConfig,
) -> Result<pbt::ProjectModel, SpecConversionError> {
    let project = pbt::ProjectModel {
        name: manifest.package.name.clone(),
        build_string_prefix: manifest.build.build_string_prefix.clone(),
        build_number: manifest.build.build_number,
        version: manifest.package.version.clone(),
        description: manifest.package.description.clone(),
        authors: manifest.package.authors.clone(),
        license: manifest.package.license.clone(),
        license_file: manifest.package.license_file.clone(),
        readme: manifest.package.readme.clone(),
        homepage: manifest.package.homepage.clone(),
        repository: manifest.package.repository.clone(),
        documentation: manifest.package.documentation.clone(),
        targets: Some(to_targets_v1(&manifest.targets, channel_config)?),
        secrets: manifest.build.secrets.clone(),
    };
    Ok(project)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rattler_conda_types::ChannelConfig;
    use rstest::rstest;

    fn some_channel_config() -> ChannelConfig {
        ChannelConfig {
            channel_alias: "http://prefix.dev".parse().unwrap(),
            root_dir: PathBuf::from("/tmp"),
        }
    }

    /// Use a macro so that the snapshot test is inlined into the function
    /// this makes insta use the name of the function as the snapshot name
    /// instead of this generic name
    macro_rules! snapshot_test {
        ($manifest_path:expr) => {{
            use std::ffi::OsStr;

            let manifest = pixi_manifest::Manifests::from_workspace_manifest_path($manifest_path)
                .expect("could not load manifest")
                .value;
            if let Some(package_manifest) = manifest.package {
                // To create different snapshot files for the same function
                let name = package_manifest
                    .provenance
                    .path
                    .parent()
                    .unwrap()
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap();

                // Convert the manifest to the project model
                let project_model =
                    super::to_project_model_v1(&package_manifest.value, &some_channel_config())
                        .unwrap();
                let mut settings = insta::Settings::clone_current();
                settings.set_snapshot_suffix(name);
                settings.bind(|| {
                    insta::assert_json_snapshot!(project_model);
                });
            }
        }};
    }

    #[rstest]
    #[test]
    fn test_conversions_v1_examples(
        #[files("../../examples/pixi-build/*/pixi.toml")] manifest_path: PathBuf,
    ) {
        snapshot_test!(manifest_path);
    }

    #[rstest]
    #[test]
    fn test_conversions_v1_docs(
        #[files("../../docs/source_files/pixi_workspaces/pixi_build/*/pixi.toml")]
        manifest_path: PathBuf,
    ) {
        snapshot_test!(manifest_path);
    }

    /// Regression test: `to_target_v1` must propagate `[package.run-constraints]`
    /// (the `SpecType::RunConstraints` bucket on `PackageTarget`) into the
    /// `pbt::Target.run_constraints` field. A previous version dropped them
    /// silently because `to_target_v1` only mapped run/host/build.
    #[test]
    fn test_to_target_v1_run_constraints() {
        use std::str::FromStr;

        use pixi_manifest::{DependencyOverwriteBehavior, PackageTarget, SpecType};
        use pixi_spec::PixiSpec;
        use rattler_conda_types::{PackageName, ParseStrictness, VersionSpec};

        use super::pbt;

        let mut package_target = PackageTarget::default();
        let constrained = PackageName::from_str("constrained").unwrap();
        let spec =
            PixiSpec::Version(VersionSpec::from_str(">=1.0", ParseStrictness::Strict).unwrap());
        package_target
            .try_add_dependency(
                &constrained,
                &spec,
                SpecType::RunConstraints,
                DependencyOverwriteBehavior::Error,
            )
            .unwrap();

        let target = super::to_target_v1(&package_target, &some_channel_config()).unwrap();

        let constraints = target
            .run_constraints
            .expect("run_constraints should be Some");
        assert_eq!(constraints.len(), 1);
        let (name, converted) = constraints.iter().next().unwrap();
        assert_eq!(name.as_str(), "constrained");
        match converted {
            pbt::PackageSpec::Binary(binary) => assert_eq!(
                binary.version.as_ref().unwrap().to_string(),
                ">=1.0",
                "expected version spec to round-trip",
            ),
            other => panic!("expected Binary spec, got {other:?}"),
        }

        // Confirm the other buckets stay empty so we know we routed only to constraints.
        assert!(target.run_dependencies.unwrap().is_empty());
        assert!(target.host_dependencies.unwrap().is_empty());
        assert!(target.build_dependencies.unwrap().is_empty());
    }
}
