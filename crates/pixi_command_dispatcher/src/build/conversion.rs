use pixi_build_types::{
    BinaryPackageSpecV1, CondaPackageMetadata, PackageSpecV1, SourcePackageSpecV1,
};
use pixi_record::{InputHash, PinnedSourceSpec, SourceRecord};
use pixi_spec::{BinarySpec, DetailedSpec, SourceLocationSpec, UrlBinarySpec};
use rattler_conda_types::{NamedChannelOrUrl, PackageName, PackageRecord};

/// Converts a [`SourcePackageSpecV1`] to a [`pixi_spec::SourceSpec`].
pub fn from_source_spec_v1(source: SourcePackageSpecV1) -> pixi_spec::SourceSpec {
    match source {
        SourcePackageSpecV1::Url(url) => pixi_spec::SourceSpec {
            location: SourceLocationSpec::Url(pixi_spec::UrlSourceSpec {
                url: url.url,
                md5: url.md5,
                sha256: url.sha256,
            }),
        },
        SourcePackageSpecV1::Git(git) => pixi_spec::SourceSpec {
            location: SourceLocationSpec::Git(pixi_spec::GitSpec {
                git: git.git,
                rev: git.rev.map(|r| match r {
                    pixi_build_frontend::types::GitReferenceV1::Branch(b) => {
                        pixi_spec::GitReference::Branch(b)
                    }
                    pixi_build_frontend::types::GitReferenceV1::Tag(t) => {
                        pixi_spec::GitReference::Tag(t)
                    }
                    pixi_build_frontend::types::GitReferenceV1::Rev(rev) => {
                        pixi_spec::GitReference::Rev(rev)
                    }
                    pixi_build_frontend::types::GitReferenceV1::DefaultBranch => {
                        pixi_spec::GitReference::DefaultBranch
                    }
                }),
                subdirectory: git.subdirectory,
            }),
        },
        SourcePackageSpecV1::Path(path) => pixi_spec::SourceSpec {
            location: SourceLocationSpec::Path(pixi_spec::PathSourceSpec {
                path: path.path.into(),
            }),
        },
    }
}

/// Converts a [`BinaryPackageSpecV1`] to a [`pixi_spec::BinarySpec`].
pub fn from_binary_spec_v1(spec: BinaryPackageSpecV1) -> pixi_spec::BinarySpec {
    match spec {
        BinaryPackageSpecV1 {
            url: Some(url),
            sha256,
            md5,
            ..
        } => BinarySpec::Url(UrlBinarySpec { url, md5, sha256 }),
        BinaryPackageSpecV1 {
            version: Some(version),
            build: None,
            build_number: None,
            file_name: None,
            channel: None,
            subdir: None,
            md5: None,
            sha256: None,
            license: None,
            url: _,
        } => BinarySpec::Version(version),
        BinaryPackageSpecV1 {
            version,
            build,
            build_number,
            file_name,
            channel,
            subdir,
            md5,
            sha256,
            license,
            url: _,
        } => BinarySpec::DetailedVersion(Box::new(DetailedSpec {
            version,
            build,
            build_number,
            file_name,
            channel: channel.map(NamedChannelOrUrl::Url),
            subdir,
            license,
            md5,
            sha256,
        })),
    }
}

/// Converts a [`PackageSpecV1`] to a [`pixi_spec::PixiSpec`].
pub fn from_package_spec_v1(source: PackageSpecV1) -> pixi_spec::PixiSpec {
    match source {
        PackageSpecV1::Source(source) => from_source_spec_v1(source).into(),
        PackageSpecV1::Binary(binary) => from_binary_spec_v1(*binary).into(),
    }
}

pub(crate) fn package_metadata_to_source_records(
    source: &PinnedSourceSpec,
    packages: &[CondaPackageMetadata],
    package: &PackageName,
    input_hash: &Option<InputHash>,
) -> Vec<SourceRecord> {
    // Convert the metadata to repodata
    let packages = packages
        .iter()
        .filter(|pkg| pkg.name == *package)
        .map(|p| {
            SourceRecord {
                input_hash: input_hash.clone(),
                source: source.clone(),
                sources: p
                    .sources
                    .iter()
                    .map(|(name, source)| (name.clone(), from_source_spec_v1(source.clone())))
                    .collect(),
                package_record: PackageRecord {
                    // We cannot now these values from the metadata because no actual package
                    // was built yet.
                    size: None,
                    sha256: None,
                    md5: None,

                    // TODO(baszalmstra): Decide if it makes sense to include the current
                    // timestamp here.
                    timestamp: None,

                    // These values are derived from the build backend values.
                    platform: p.subdir.only_platform().map(ToString::to_string),
                    arch: p.subdir.arch().as_ref().map(ToString::to_string),

                    // These values are passed by the build backend
                    name: p.name.clone(),
                    build: p.build.clone(),
                    version: p.version.clone(),
                    build_number: p.build_number,
                    license: p.license.clone(),
                    subdir: p.subdir.to_string(),
                    license_family: p.license_family.clone(),
                    noarch: p.noarch,
                    constrains: p.constraints.iter().map(|c| c.to_string()).collect(),
                    depends: p.depends.iter().map(|c| c.to_string()).collect(),

                    // These are deprecated and no longer used.
                    features: None,
                    track_features: vec![],
                    legacy_bz2_md5: None,
                    legacy_bz2_size: None,
                    python_site_packages_path: None,

                    // TODO(baszalmstra): Add support for these.
                    purls: None,

                    // These are not important at this point.
                    run_exports: None,
                    experimental_extra_depends: Default::default(),
                },
            }
        })
        .collect();
    packages
}
