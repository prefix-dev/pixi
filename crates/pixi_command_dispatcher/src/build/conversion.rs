use pixi_build_types::{BinaryPackageSpec, SourcePackageLocationSpec, SourcePackageSpec};
use pixi_spec::{BinarySpec, DetailedSpec, UrlBinarySpec};
use rattler_conda_types::NamedChannelOrUrl;

/// Converts a [`SourcePackageSpec`] to a [`pixi_spec::SourceSpec`].
pub fn from_source_spec_v1(source: SourcePackageSpec) -> pixi_spec::SourceSpec {
    let SourcePackageSpec {
        location,
        version,
        build,
        build_number,
        subdir,
        license,
    } = source;
    let location = from_source_package_location_spec(location);
    pixi_spec::SourceSpec {
        location,
        version,
        build,
        build_number,
        subdir,
        license,
        extras: None,
        namespace: None,
        condition: None,
    }
}

pub fn from_source_package_location_spec(
    spec: SourcePackageLocationSpec,
) -> pixi_spec::SourceLocationSpec {
    match spec {
        SourcePackageLocationSpec::Url(url) => {
            pixi_spec::SourceLocationSpec::Url(pixi_spec::UrlSourceSpec {
                url: url.url,
                md5: url.md5,
                sha256: url.sha256,
                subdirectory: url
                    .subdirectory
                    .and_then(|s| pixi_spec::Subdirectory::try_from(s).ok())
                    .unwrap_or_default(),
            })
        }

        SourcePackageLocationSpec::Git(git) => {
            pixi_spec::SourceLocationSpec::Git(pixi_spec::GitSpec {
                git: git.git,
                rev: git.rev.map(|r| match r {
                    pixi_build_frontend::types::GitReference::Branch(b) => {
                        pixi_spec::GitReference::Branch(b)
                    }
                    pixi_build_frontend::types::GitReference::Tag(t) => {
                        pixi_spec::GitReference::Tag(t)
                    }
                    pixi_build_frontend::types::GitReference::Rev(rev) => {
                        pixi_spec::GitReference::Rev(rev)
                    }
                    pixi_build_frontend::types::GitReference::DefaultBranch => {
                        pixi_spec::GitReference::DefaultBranch
                    }
                }),
                subdirectory: git
                    .subdirectory
                    .and_then(|s| pixi_spec::Subdirectory::try_from(s).ok())
                    .unwrap_or_default(),
            })
        }

        SourcePackageLocationSpec::Path(path) => {
            pixi_spec::SourceLocationSpec::Path(pixi_spec::PathSourceSpec {
                path: path.path.into(),
            })
        }
    }
}

/// Converts a [`BinaryPackageSpec`] to a [`pixi_spec::BinarySpec`].
pub fn from_binary_spec_v1(spec: BinaryPackageSpec) -> pixi_spec::BinarySpec {
    match spec {
        BinaryPackageSpec {
            url: Some(url),
            sha256,
            md5,
            ..
        } => BinarySpec::Url(UrlBinarySpec { url, md5, sha256 }),
        BinaryPackageSpec {
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
        BinaryPackageSpec {
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
