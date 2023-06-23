use itertools::Either;
use rattler_conda_types::package::ArchiveType;
use rattler_conda_types::{ChannelInfo, PackageRecord, Platform, RepoData, Version};
use std::iter;

#[derive(Default, Clone, Debug)]
pub struct ChannelBuilder {
    subdirs: Vec<SubdirBuilder>,
}

impl ChannelBuilder {
    pub fn with_subdir(mut self, subdir: SubdirBuilder) -> Self {
        self.subdirs.push(subdir);
        self
    }

    /// Writes the channel to disk
    pub async fn write_to_disk(&self) -> anyhow::Result<tempfile::TempDir> {
        let dir = tempfile::TempDir::new()?;

        let empty_noarch = SubdirBuilder::new(Platform::NoArch);

        let subdirs = if self
            .subdirs
            .iter()
            .any(|subdir| subdir.platform == Platform::NoArch)
        {
            Either::Left(self.subdirs.iter())
        } else {
            Either::Right(self.subdirs.iter().chain(iter::once(&empty_noarch)))
        };

        for subdir in subdirs {
            let subdir_path = dir.path().join(subdir.platform.as_str());

            let repodata = RepoData {
                info: Some(ChannelInfo {
                    subdir: subdir.platform.to_string(),
                }),
                packages: subdir
                    .packages
                    .iter()
                    .filter(|pkg| pkg.archive_type == ArchiveType::TarBz2)
                    .map(|pkg| pkg.as_package_record(subdir.platform))
                    .collect(),
                conda_packages: subdir
                    .packages
                    .iter()
                    .filter(|pkg| pkg.archive_type == ArchiveType::Conda)
                    .map(|pkg| pkg.as_package_record(subdir.platform))
                    .collect(),
                removed: Default::default(),
                version: Some(1),
            };
            let repodata_str = serde_json::to_string_pretty(&repodata)?;

            tokio::fs::create_dir_all(&subdir_path).await?;
            tokio::fs::write(subdir_path.join("repodata.json"), repodata_str).await?;
        }

        Ok(dir)
    }
}

#[derive(Clone, Debug)]
pub struct SubdirBuilder {
    packages: Vec<PackageBuilder>,
    platform: Platform,
}

impl SubdirBuilder {
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            packages: vec![],
        }
    }

    pub fn with_package(mut self, package: PackageBuilder) -> Self {
        self.packages.push(package);
        self
    }
}

#[derive(Clone, Debug)]
pub struct PackageBuilder {
    name: String,
    version: Version,
    build_string: String,
    depends: Vec<String>,
    archive_type: ArchiveType,
}

impl PackageBuilder {
    pub fn new(package: impl Into<String>, version: &str) -> Self {
        Self {
            name: package.into(),
            version: version.parse().expect("invalid version"),
            build_string: String::from("0"),
            depends: vec![],
            archive_type: ArchiveType::Conda,
        }
    }

    #[allow(dead_code)]
    pub fn with_build_string(mut self, build_string: impl Into<String>) -> Self {
        self.build_string = build_string.into();
        debug_assert!(!self.build_string.is_empty());
        self
    }

    #[allow(dead_code)]
    pub fn with_archive_type(mut self, archive_type: ArchiveType) -> Self {
        self.archive_type = archive_type;
        self
    }

    pub fn with_dependency(mut self, spec: impl Into<String>) -> Self {
        self.depends.push(spec.into());
        self
    }

    pub fn as_package_record(&self, platform: Platform) -> (String, PackageRecord) {
        // Construct the package filename
        let filename = format!(
            "{}-{}-{}.{}",
            self.name.to_lowercase(),
            &self.version,
            &self.build_string,
            match self.archive_type {
                ArchiveType::TarBz2 => "tar.bz2",
                ArchiveType::Conda => "conda",
            }
        );

        // Construct the record
        let record = PackageRecord {
            // TODO: This is wrong, we should extract this from platform
            arch: None,
            platform: None,

            build: self.build_string.clone(),
            build_number: 0,
            constrains: vec![],
            depends: self.depends.clone(),
            features: None,
            legacy_bz2_md5: None,
            legacy_bz2_size: None,
            license: None,
            license_family: None,
            md5: Some(rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(&filename)),
            name: self.name.clone(),
            noarch: Default::default(),
            sha256: Some(rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(
                &filename,
            )),
            size: None,
            subdir: platform.to_string(),
            timestamp: None,
            track_features: vec![],
            version: self.version.clone(),
        };

        (filename, record)
    }
}
