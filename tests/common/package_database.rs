//! This modules defines [`PackageDatabase`], a struct that holds a bunch of easily constructable
//! package definitions. Using this struct it becomes easier to generate controllable fake repodata.

// There are a bunch of functions that remain unused in tests but might be useful in the future.
#![allow(dead_code)]

use itertools::Itertools;
use rattler_conda_types::{
    package::ArchiveType, ChannelInfo, PackageRecord, Platform, RepoData,
    VersionWithSource,
};
use std::{collections::HashSet, path::Path};

/// A database of packages
#[derive(Default, Clone, Debug)]
pub struct PackageDatabase {
    packages: Vec<Package>,
}

impl PackageDatabase {
    /// Adds a package to the database
    pub fn with_package(mut self, package: Package) -> Self {
        self.packages.push(package);
        self
    }

    /// Adds a package to the database
    pub fn add_package(&mut self, package: Package) {
        self.packages.push(package);
    }

    /// Writes the repodata of this instance to the specified channel directory
    pub async fn write_repodata(&self, channel_path: &Path) -> anyhow::Result<()> {
        let mut platforms = self.platforms();

        // Make sure NoArch is always included
        if !platforms.contains(&Platform::NoArch) {
            platforms.insert(Platform::NoArch);
        }

        // Make sure the current platform is included
        let current_platform = Platform::current();
        if !platforms.contains(&current_platform) {
            platforms.insert(current_platform);
        }

        for platform in platforms {
            let subdir_path = channel_path.join(platform.as_str());

            let repodata = RepoData {
                info: Some(ChannelInfo {
                    subdir: platform.to_string(),
                }),
                packages: self
                    .packages_by_platform(platform)
                    .filter(|pkg| pkg.archive_type == ArchiveType::TarBz2)
                    .map(|pkg| (pkg.file_name(), pkg.package_record.clone()))
                    .sorted_by(|a, b| a.0.cmp(&b.0))
                    .collect(),
                conda_packages: self
                    .packages_by_platform(platform)
                    .filter(|pkg| pkg.archive_type == ArchiveType::Conda)
                    .map(|pkg| (pkg.file_name(), pkg.package_record.clone()))
                    .sorted_by(|a, b| a.0.cmp(&b.0))
                    .collect(),
                removed: Default::default(),
                version: Some(1),
            };
            let repodata_str = serde_json::to_string_pretty(&repodata)?;

            tokio::fs::create_dir_all(&subdir_path).await?;
            tokio::fs::write(subdir_path.join("repodata.json"), repodata_str).await?;
        }

        Ok(())
    }

    /// Returns all packages for the specified platform.
    pub fn packages_by_platform(
        &self,
        platform: Platform,
    ) -> impl Iterator<Item = &'_ Package> + '_ {
        self.packages
            .iter()
            .filter(move |pkg| pkg.subdir == platform)
    }

    /// Returns all the platforms that this database has packages for
    pub fn platforms(&self) -> HashSet<Platform> {
        self.packages.iter().map(|pkg| pkg.subdir).collect()
    }
}

/// Description of a package.
#[derive(Clone, Debug)]
pub struct Package {
    package_record: PackageRecord,
    subdir: Platform,
    archive_type: ArchiveType,
}

// Implement `AsRef` for a `PackageRecord` allows using `Package` in a number of algorithms used in
// `rattler_conda_types`.
impl AsRef<PackageRecord> for Package {
    fn as_ref(&self) -> &PackageRecord {
        &self.package_record
    }
}

/// A builder for a [`Package`]
pub struct PackageBuilder {
    name: String,
    version: VersionWithSource,
    build: Option<String>,
    build_number: Option<u64>,
    depends: Vec<String>,
    subdir: Option<Platform>,
    archive_type: ArchiveType,
}

impl Package {
    /// Constructs a new [`Package`].
    pub fn build(name: impl ToString, version: &str) -> PackageBuilder {
        PackageBuilder {
            name: name.to_string(),
            version: version.parse().unwrap(),
            build: None,
            build_number: None,
            depends: vec![],
            subdir: None,
            archive_type: ArchiveType::Conda,
        }
    }

    /// Returns the file name for this package.
    pub fn file_name(&self) -> String {
        format!(
            "{}-{}-{}{}",
            self.package_record.name,
            self.package_record.version,
            self.package_record.build,
            self.archive_type.extension()
        )
    }
}

impl PackageBuilder {
    /// Set the build string of this package
    pub fn with_build(mut self, build: impl ToString) -> Self {
        self.build = Some(build.to_string());
        self
    }

    /// Set the build string of this package
    pub fn with_build_number(mut self, build_number: u64) -> Self {
        self.build_number = Some(build_number);
        self
    }

    /// Set the build string of this package
    pub fn with_dependency(mut self, dependency: impl ToString) -> Self {
        self.depends.push(dependency.to_string());
        self
    }

    /// Explicitly set the platform of this package
    pub fn with_subdir(mut self, subdir: Platform) -> Self {
        self.subdir = Some(subdir);
        self
    }

    /// Set the archive type of this package
    pub fn with_archive_type(mut self, archive_type: ArchiveType) -> Self {
        self.archive_type = archive_type;
        self
    }

    /// Finish construction of the package
    pub fn finish(self) -> Package {
        let subdir = self.subdir.unwrap_or(Platform::NoArch);
        let build_number = self.build_number.unwrap_or(0);
        let build = self.build.unwrap_or_else(|| format!("{build_number}"));
        let hash = format!(
            "{}-{}-{}{}",
            &self.name,
            &self.version,
            &build,
            self.archive_type.extension()
        );
        let md5 = rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(&hash);
        let sha256 = rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&hash);
        Package {
            package_record: PackageRecord {
                arch: None,
                build,
                build_number,
                constrains: vec![],
                depends: self.depends,
                features: None,
                legacy_bz2_md5: None,
                legacy_bz2_size: None,
                license: None,
                license_family: None,
                md5: Some(md5),
                name: self.name,
                noarch: Default::default(),
                platform: None,
                sha256: Some(sha256),
                size: None,
                subdir: subdir.to_string(),
                timestamp: None,
                track_features: vec![],
                version: self.version,
            },
            subdir,
            archive_type: self.archive_type,
        }
    }
}
