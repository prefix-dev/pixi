//! Conversion functions from `pixi_spec` types to `pixi_build_types` types.
//! these are used to convert the `pixi_spec` types to the `pixi_build_types` types
//! we want to keep the conversion here, as we do not want `pixi_build_types` to depend on `pixi_spec`
//!
//! This will mostly be boilerplate conversions but some of these are a bit more interesting

// Namespace to pbt, *please use* exclusively so we do not get confused between the two different types
use indexmap::IndexMap;
use pixi_build_types as pbt;
use pixi_manifest::{PackageManifest, PackageTarget, TargetSelector, Targets};
use pixi_spec::{PixiSpec, Reference};
use rattler_conda_types::PackageName;

/// Conversion from a `PixiSpec` to a `pbt::PixiSpecV1`.
fn to_pixi_spec_v1(spec: &PixiSpec) -> pbt::PixiSpecV1 {
    match spec {
        // We consider both `Version` and `DetailedVersion` to use
        // the NamelessMatchSpecV1 variant
        PixiSpec::Version(version_spec) => pbt::PixiSpecV1::DetailedVersion(version_spec.into()),
        PixiSpec::DetailedVersion(detailed_spec) => {
            pbt::PixiSpecV1::DetailedVersion(pbt::NamelessMatchSpecV1 {
                version: detailed_spec.version.clone(),
                build: detailed_spec.build.clone(),
                build_number: detailed_spec.build_number.clone(),
                file_name: detailed_spec.file_name.clone(),
                channel: detailed_spec
                    .channel
                    .as_ref()
                    .map(|c| c.to_string())
                    .clone(),
                subdir: detailed_spec.subdir.clone(),
                namespace: None,
                md5: detailed_spec.md5.map(Into::into),
                sha256: detailed_spec.sha256.map(Into::into),
                url: None,
            })
        }
        PixiSpec::Url(url_spec) => pbt::PixiSpecV1::Url(pbt::UrlSpecV1 {
            url: url_spec.url.clone(),
            md5: url_spec.md5.map(Into::into),
            sha256: url_spec.sha256.map(Into::into),
        }),
        PixiSpec::Git(git_spec) => pbt::PixiSpecV1::Git(pbt::GitSpecV1 {
            git: git_spec.git.clone(),
            rev: git_spec.rev.clone().map(|r| match r {
                Reference::Branch(b) => pbt::GitReferenceV1::Branch(b.clone()),
                Reference::Tag(t) => pbt::GitReferenceV1::Tag(t.clone()),
                Reference::Rev(rev) => pbt::GitReferenceV1::Rev(rev.clone()),
                Reference::DefaultBranch => pbt::GitReferenceV1::DefaultBranch,
            }),
            subdirectory: git_spec.subdirectory.clone(),
        }),
        PixiSpec::Path(path_spec) => pbt::PixiSpecV1::Path(pbt::PathSpecV1 {
            path: path_spec.path.to_string(),
        }),
    }
}

/// Converts an iterator of `PackageName` and `PixiSpec` to a `IndexMap<String, pbt::PixiSpecV1>`.
fn to_pbt_dependencies<'a>(
    iter: impl Iterator<Item = (&'a PackageName, &'a PixiSpec)>,
) -> IndexMap<String, pbt::PixiSpecV1> {
    iter.map(|(k, v)| (k.as_source().to_string(), to_pixi_spec_v1(v)))
        .collect()
}

/// Converts a [`PackageTarget`] to a [`pbt::TargetV1`].
fn to_target_v1(target: &PackageTarget) -> pbt::TargetV1 {
    // Difference for us is that [`pbt::TargetV1`] has split the host, run and build dependencies
    // into separate fields, so we need to split them up here
    pbt::TargetV1 {
        host_dependencies: target
            .host_dependencies()
            .map(|deps| deps.iter())
            .map(to_pbt_dependencies)
            .unwrap_or_default(),
        build_dependencies: target
            .build_dependencies()
            .map(|deps| deps.iter())
            .map(to_pbt_dependencies)
            .unwrap_or_default(),
        run_dependencies: target
            .run_dependencies()
            .map(|deps| deps.iter())
            .map(to_pbt_dependencies)
            .unwrap_or_default(),
    }
}

fn to_target_selector_v1(selector: &TargetSelector) -> pbt::TargetSelectorV1 {
    match selector {
        TargetSelector::Platform(platform) => pbt::TargetSelectorV1::Platform(platform.to_string()),
        TargetSelector::Unix => pbt::TargetSelectorV1::Unix,
        TargetSelector::Linux => pbt::TargetSelectorV1::Linux,
        TargetSelector::Win => pbt::TargetSelectorV1::Win,
        TargetSelector::MacOs => pbt::TargetSelectorV1::MacOs,
    }
}

fn to_targets_v1(targets: &Targets<PackageTarget>) -> pbt::TargetsV1 {
    pbt::TargetsV1 {
        default_target: to_target_v1(targets.default()),
        targets: targets
            .iter()
            .filter_map(|(k, v)| {
                v.map(|selector| (to_target_selector_v1(selector), to_target_v1(k)))
            })
            .collect(),
    }
}

/// Converts a [`PackageManifest`] to a [`pbt::ProjectModelV1`].
pub fn to_project_model_v1(manifest: &PackageManifest) -> pbt::ProjectModelV1 {
    pbt::ProjectModelV1 {
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
        configuration: serde_json::Value::Null,
        targets: to_targets_v1(&manifest.targets),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use std::path::PathBuf;

    /// Use a macro so that the snapshot test is inlined into the function
    /// this makes insta use the name of the function as the snapshot name
    /// instead of this generic name
    macro_rules! snapshot_test {
        ($manifest_path:expr) => {{
            use std::ffi::OsStr;

            let manifest = pixi_manifest::Manifest::from_path(&$manifest_path)
                .expect("could not load manifest");
            if let Some(package_manifest) = manifest.package {
                // To create different snapshot files for the same function
                let name = $manifest_path
                    .parent()
                    .unwrap()
                    .file_name()
                    .and_then(OsStr::to_str)
                    .unwrap();

                // Convert the manifest to the project model
                let project_model = super::to_project_model_v1(&package_manifest);
                let mut settings = insta::Settings::clone_current();
                settings.set_snapshot_suffix(name);
                settings.bind(|| {
                    insta::assert_yaml_snapshot!(project_model);
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
        #[files("../../docs/source_files/pixi_projects/pixi_build_*/pixi.toml")]
        manifest_path: PathBuf,
    ) {
        snapshot_test!(manifest_path);
    }
}
