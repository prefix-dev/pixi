//! Conversion functions from `pixi_spec` types to `pixi_build_types` types.
//! these are used to convert the `pixi_spec` types to the `pixi_build_types`
//! types we want to keep the conversion here, as we do not want
//! `pixi_build_types` to depend on `pixi_spec`
//!
//! This will mostly be boilerplate conversions but some of these are a bit more
//! interesting

use std::hash::{Hash, Hasher};

use ordermap::OrderMap;
// Namespace to pbt, *please use* exclusively so we do not get confused between the two
// different types
use pixi_build_types::{self as pbt, ProjectModelV1};

use pixi_manifest::{PackageManifest, PackageTarget, TargetSelector, Targets};
use pixi_spec::{GitReference, PixiSpec, SpecConversionError};
use rattler_conda_types::{ChannelConfig, NamelessMatchSpec, PackageName};
use xxhash_rust::xxh3::Xxh3;

/// Conversion from a `PixiSpec` to a `pbt::PixiSpecV1`.
fn to_pixi_spec_v1(
    spec: &PixiSpec,
    channel_config: &ChannelConfig,
) -> Result<pbt::PackageSpecV1, SpecConversionError> {
    // Convert into source or binary
    let source_or_binary = spec.clone().into_source_or_binary();
    // Convert into correct type for pixi
    let pbt_spec = match source_or_binary {
        itertools::Either::Left(source) => {
            let source = match source.location {
                pixi_spec::SourceLocationSpec::Url(url_source_spec) => {
                    let pixi_spec::UrlSourceSpec { url, md5, sha256 } = url_source_spec;
                    pbt::SourcePackageSpecV1::Url(pbt::UrlSpecV1 { url, md5, sha256 })
                }
                pixi_spec::SourceLocationSpec::Git(git_spec) => {
                    let pixi_spec::GitSpec {
                        git,
                        rev,
                        subdirectory,
                    } = git_spec;
                    pbt::SourcePackageSpecV1::Git(pbt::GitSpecV1 {
                        git,
                        rev: rev.map(|r| match r {
                            GitReference::Branch(b) => pbt::GitReferenceV1::Branch(b),
                            GitReference::Tag(t) => pbt::GitReferenceV1::Tag(t),
                            GitReference::Rev(rev) => pbt::GitReferenceV1::Rev(rev),
                            GitReference::DefaultBranch => pbt::GitReferenceV1::DefaultBranch,
                        }),
                        subdirectory,
                    })
                }
                pixi_spec::SourceLocationSpec::Path(path_source_spec) => {
                    pbt::SourcePackageSpecV1::Path(pbt::PathSpecV1 {
                        path: path_source_spec.path.to_string(),
                    })
                }
            };
            pbt::PackageSpecV1::Source(source)
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
            } = binary.try_into_nameless_match_spec(channel_config)?;
            pbt::PackageSpecV1::Binary(Box::new(pbt::BinaryPackageSpecV1 {
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
            }))
        }
    };
    Ok(pbt_spec)
}

/// Converts an iterator of `PackageName` and `PixiSpec` to a `IndexMap<String,
/// pbt::PixiSpecV1>`.
fn to_pbt_dependencies<'a>(
    iter: impl Iterator<Item = (&'a PackageName, &'a PixiSpec)>,
    channel_config: &ChannelConfig,
) -> Result<OrderMap<pbt::SourcePackageName, pbt::PackageSpecV1>, SpecConversionError> {
    iter.map(|(name, spec)| {
        let converted = to_pixi_spec_v1(spec, channel_config)?;
        Ok((name.as_normalized().to_string(), converted))
    })
    .collect()
}

/// Converts a [`PackageTarget`] to a [`pbt::TargetV1`].
fn to_target_v1(
    target: &PackageTarget,
    channel_config: &ChannelConfig,
) -> Result<pbt::TargetV1, SpecConversionError> {
    // Difference for us is that [`pbt::TargetV1`] has split the host, run and build
    // dependencies into separate fields, so we need to split them up here
    Ok(pbt::TargetV1 {
        host_dependencies: Some(
            target
                .host_dependencies()
                .map(|deps| to_pbt_dependencies(deps.iter(), channel_config))
                .transpose()?
                .unwrap_or_default(),
        ),
        build_dependencies: Some(
            target
                .build_dependencies()
                .map(|deps| to_pbt_dependencies(deps.iter(), channel_config))
                .transpose()?
                .unwrap_or_default(),
        ),
        run_dependencies: Some(
            target
                .run_dependencies()
                .map(|deps| to_pbt_dependencies(deps.iter(), channel_config))
                .transpose()?
                .unwrap_or_default(),
        ),
    })
}

pub fn to_target_selector_v1(selector: &TargetSelector) -> pbt::TargetSelectorV1 {
    match selector {
        TargetSelector::Platform(platform) => pbt::TargetSelectorV1::Platform(platform.to_string()),
        TargetSelector::Unix => pbt::TargetSelectorV1::Unix,
        TargetSelector::Linux => pbt::TargetSelectorV1::Linux,
        TargetSelector::Win => pbt::TargetSelectorV1::Win,
        TargetSelector::MacOs => pbt::TargetSelectorV1::MacOs,
    }
}

fn to_targets_v1(
    targets: &Targets<PackageTarget>,
    channel_config: &ChannelConfig,
) -> Result<pbt::TargetsV1, SpecConversionError> {
    let selected_targets = targets
        .iter()
        .filter_map(|(k, v)| {
            v.map(|selector| {
                to_target_v1(k, channel_config)
                    .map(|target| (to_target_selector_v1(selector), target))
            })
        })
        .collect::<Result<OrderMap<pbt::TargetSelectorV1, pbt::TargetV1>, _>>()?;

    Ok(pbt::TargetsV1 {
        default_target: Some(to_target_v1(targets.default(), channel_config)?),
        targets: Some(selected_targets),
    })
}

/// Converts a [`PackageManifest`] to a [`pbt::ProjectModelV1`].
pub fn to_project_model_v1(
    manifest: &PackageManifest,
    channel_config: &ChannelConfig,
) -> Result<pbt::ProjectModelV1, SpecConversionError> {
    let project = pbt::ProjectModelV1 {
        name: manifest.package.name.clone(),
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
    };
    Ok(project)
}

/// This function is used to calculate a stable hash for the project model
/// This is used to trigger cache invalidation if the project model changes
pub fn compute_project_model_hash(project_model: &ProjectModelV1) -> Vec<u8> {
    let mut hasher = Xxh3::new();
    project_model.hash(&mut hasher);
    hasher.finish().to_ne_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use pixi_build_types::VersionedProjectModel;
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
                let project_model: VersionedProjectModel =
                    super::to_project_model_v1(&package_manifest.value, &some_channel_config())
                        .unwrap()
                        .into();
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
}
