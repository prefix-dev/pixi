//! This modules defines [`MockRepoData`], a struct that holds a bunch of easily constructable
//! package definitions. Using this struct it becomes easier to generate controllable fake repodata.

use chrono::{DateTime, Utc};
use miette::IntoDiagnostic;
use rattler_conda_types::package::ArchiveIdentifier;
use rattler_conda_types::{
    ChannelInfo, PackageName, PackageRecord, PackageUrl, Platform, RepoData, VersionWithSource,
    package::{
        CondaArchiveType, DistArchiveIdentifier, IndexJson, PathType, PathsEntry, PathsJson,
        RunExportsJson,
    },
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tempfile::TempDir;
use url::Url;

pub struct LocalChannel {
    dir: TempDir,
    _db: MockRepoData,
}

impl LocalChannel {
    pub fn url(&self) -> Url {
        Url::from_file_path(self.dir.path()).unwrap()
    }
}

/// A database of packages
#[derive(Default, Clone, Debug)]
pub struct MockRepoData {
    packages: Vec<Package>,
}

impl MockRepoData {
    /// Adds a package to the database
    pub fn with_package(mut self, package: Package) -> Self {
        self.packages.push(package);
        self
    }

    /// Adds a package to the database
    pub fn add_package(&mut self, package: Package) {
        self.packages.push(package);
    }

    /// Writes the repodata of this instance to the specified channel directory.
    /// For packages with `materialize` enabled, actual .conda files will be created.
    pub async fn write_repodata(&self, channel_path: &Path) -> miette::Result<()> {
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

            // Create the subdir first
            tokio::fs::create_dir_all(&subdir_path)
                .await
                .into_diagnostic()?;

            // Process packages and create materialized ones if needed
            let mut tar_bz2_packages = Vec::new();
            let mut conda_packages = Vec::new();

            for pkg in self.packages_by_platform(platform) {
                let identifier = pkg.identifier();
                let package_record = if pkg.materialize {
                    // Create the actual .conda file and get the real hashes
                    let package_path = subdir_path.join(identifier.to_file_name());
                    let (sha256, md5, size) =
                        create_conda_package(pkg, &package_path).into_diagnostic()?;

                    // Create updated package record with real hashes
                    let mut updated_record = pkg.package_record.clone();
                    updated_record.sha256 = Some(sha256);
                    updated_record.md5 = Some(md5);
                    updated_record.size = Some(size);

                    updated_record
                } else {
                    pkg.package_record.clone()
                };

                match pkg.archive_type {
                    CondaArchiveType::TarBz2 => tar_bz2_packages.push((identifier, package_record)),
                    CondaArchiveType::Conda => conda_packages.push((identifier, package_record)),
                }
            }

            // Sort packages by filename for reproducibility
            tar_bz2_packages.sort_by(|a, b| a.0.cmp(&b.0));
            conda_packages.sort_by(|a, b| a.0.cmp(&b.0));

            let repodata = RepoData {
                info: Some(ChannelInfo {
                    subdir: Some(platform.to_string()),
                    base_url: None,
                }),
                packages: tar_bz2_packages.into_iter().collect(),
                conda_packages: conda_packages.into_iter().collect(),
                removed: Default::default(),
                version: Some(1),
                experimental_whl_packages: Default::default(),
            };
            let repodata_str = serde_json::to_string_pretty(&repodata).into_diagnostic()?;

            tokio::fs::write(subdir_path.join("repodata.json"), repodata_str)
                .await
                .into_diagnostic()?;
        }

        Ok(())
    }

    /// Converts this database into a local channel which can be referenced by a pixi project.
    pub async fn into_channel(self) -> miette::Result<LocalChannel> {
        let dir = TempDir::new().into_diagnostic()?;
        self.write_repodata(dir.path()).await?;
        Ok(LocalChannel { dir, _db: self })
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
    pub package_record: PackageRecord,
    subdir: Platform,
    archive_type: CondaArchiveType,
    /// If true, a materialized .conda file will be created for this package
    materialize: bool,
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
    archive_type: CondaArchiveType,
    timestamp: Option<DateTime<Utc>>,
    md5: Option<String>,
    sha256: Option<String>,
    purls: Option<std::collections::BTreeSet<PackageUrl>>,
    materialize: bool,
    run_exports: Option<RunExportsJson>,
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
            archive_type: CondaArchiveType::Conda,
            timestamp: None,
            sha256: None,
            md5: None,
            purls: None,
            materialize: false,
            // Default to empty run_exports to prevent the gateway from trying to
            // extract run_exports from the actual conda file, which doesn't exist
            // for non-materialized mock packages.
            run_exports: Some(RunExportsJson::default()),
        }
    }

    /// Returns whether this package should be materialized as a .conda file
    pub fn should_materialize(&self) -> bool {
        self.materialize
    }

    /// Returns the file name for this package.
    pub fn identifier(&self) -> DistArchiveIdentifier {
        DistArchiveIdentifier {
            identifier: ArchiveIdentifier {
                name: self.package_record.name.as_source().to_string(),
                version: self.package_record.version.to_string(),
                build_string: self.package_record.build.clone(),
            },
            archive_type: self.archive_type.into(),
        }
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
    pub fn with_archive_type(mut self, archive_type: CondaArchiveType) -> Self {
        self.archive_type = archive_type;
        self
    }

    /// Attach a PyPI purl for this conda package so Pixi treats it as a Python package.
    /// The version used will be the conda record version (fallback if purl has none).
    pub fn with_pypi_purl(mut self, pypi_name: impl AsRef<str>) -> Self {
        let purl = PackageUrl::builder(String::from("pypi"), pypi_name.as_ref().to_string())
            .build()
            .expect("valid pypi package url");
        match &mut self.purls {
            Some(v) => {
                v.insert(purl);
            }
            None => {
                let mut s = std::collections::BTreeSet::new();
                s.insert(purl);
                self.purls = Some(s);
            }
        }
        self
    }

    /// Sets the timestamp of the package.
    pub fn with_timestamp(mut self, timestamp: DateTime<Utc>) -> Self {
        self.timestamp = Some(timestamp);
        self
    }

    pub fn with_hashes(mut self, sha256: &str, md5: &str) -> Self {
        self.sha256 = Some(sha256.to_string());
        self.md5 = Some(md5.to_string());
        self
    }

    /// Enable materialization for this package.
    /// When enabled, a real .conda file will be created containing index.json and paths.json
    pub fn with_materialize(mut self, materialize: bool) -> Self {
        self.materialize = materialize;
        self
    }

    /// Set the run exports for this package.
    /// Run exports propagate dependencies from host to run.
    pub fn with_run_exports(mut self, run_exports: RunExportsJson) -> Self {
        self.run_exports = Some(run_exports);
        self
    }

    /// Finish construction of the package
    pub fn finish(self) -> Package {
        let subdir = self.subdir.unwrap_or(Platform::NoArch);
        let build_number = self.build_number.unwrap_or(0);
        let build = self.build.unwrap_or_else(|| format!("{build_number}"));
        let (sha256, md5) = match (self.sha256, self.md5) {
            (Some(sha256), Some(md5)) => {
                let sha256 =
                    rattler_digest::parse_digest_from_hex::<rattler_digest::Sha256>(&sha256)
                        .expect("Invalid sha256 hash format");
                let md5 = rattler_digest::parse_digest_from_hex::<rattler_digest::Md5>(&md5)
                    .expect("Invalid md5 hash format");
                (Some(sha256), Some(md5))
            }
            (None, None) => {
                // Calculate a random wrong hash for snapshot tests
                let hash = format!(
                    "{}-{}-{}{}",
                    &self.name,
                    &self.version,
                    &build,
                    self.archive_type.extension()
                );
                let md5 = rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(&hash);
                let sha256 = rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&hash);
                (Some(sha256), Some(md5))
            }
            _ => panic!("Either both sha256 and md5 should be set or none of them"),
        };

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
                md5,
                name: PackageName::new_unchecked(self.name),
                noarch: Default::default(),
                platform: None,
                sha256,
                size: None,
                subdir: subdir.to_string(),
                timestamp: self.timestamp.map(Into::into),
                track_features: vec![],
                version: self.version,
                purls: self.purls,
                run_exports: self.run_exports.clone(),
                python_site_packages_path: None,
                experimental_extra_depends: Default::default(),
            },
            subdir,
            archive_type: self.archive_type,
            materialize: self.materialize,
        }
    }
}

/// Creates a materialized .conda package file at the specified path.
///
/// This function creates a minimal but valid .conda archive containing:
/// - info/index.json - package metadata
/// - info/paths.json - list of files in the package
///
/// The package record's hash fields will be updated with the actual file hashes.
pub fn create_conda_package(
    package: &Package,
    output_path: &Path,
) -> Result<(rattler_digest::Sha256Hash, rattler_digest::Md5Hash, u64), std::io::Error> {
    use rattler_conda_types::compression_level::CompressionLevel;

    // Create a temporary directory to stage the package contents
    let temp_dir = tempfile::tempdir()?;
    let info_dir = temp_dir.path().join("info");
    fs_err::create_dir_all(&info_dir)?;

    // Create index.json
    let index_json = IndexJson {
        arch: None,
        build: package.package_record.build.clone(),
        build_number: package.package_record.build_number,
        constrains: package.package_record.constrains.clone(),
        depends: package.package_record.depends.clone(),
        experimental_extra_depends: package.package_record.experimental_extra_depends.clone(),
        features: package.package_record.features.clone(),
        license: package.package_record.license.clone(),
        license_family: package.package_record.license_family.clone(),
        name: package.package_record.name.clone(),
        noarch: package.package_record.noarch,
        platform: package.package_record.platform.clone(),
        purls: package.package_record.purls.clone(),
        python_site_packages_path: package.package_record.python_site_packages_path.clone(),
        subdir: Some(package.subdir.to_string()),
        timestamp: package.package_record.timestamp,
        track_features: package.package_record.track_features.clone(),
        version: package.package_record.version.clone(),
    };

    let index_json_content = serde_json::to_string_pretty(&index_json)?;
    let index_json_path = info_dir.join("index.json");
    fs_err::write(&index_json_path, &index_json_content)?;

    // Create paths.json (minimal - just containing the index.json entry)
    let index_json_bytes = index_json_content.as_bytes();
    let index_json_sha256 =
        rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(index_json_bytes);

    let paths_json = PathsJson {
        paths: vec![PathsEntry {
            relative_path: PathBuf::from("info/index.json"),
            no_link: false,
            path_type: PathType::HardLink,
            prefix_placeholder: None,
            sha256: Some(index_json_sha256),
            size_in_bytes: Some(index_json_bytes.len() as u64),
        }],
        paths_version: 1,
    };

    let paths_json_content = serde_json::to_string_pretty(&paths_json)?;
    let paths_json_path = info_dir.join("paths.json");
    fs_err::write(&paths_json_path, &paths_json_content)?;

    // Collect paths to include in the package
    let mut paths = vec![info_dir.join("index.json"), info_dir.join("paths.json")];

    // Create run_exports.json if the package has run exports
    if let Some(run_exports) = &package.package_record.run_exports
        && !run_exports.is_empty()
    {
        let run_exports_content = serde_json::to_string_pretty(run_exports)?;
        let run_exports_path = info_dir.join("run_exports.json");
        fs_err::write(&run_exports_path, &run_exports_content)?;
        paths.push(run_exports_path);
    }

    // Create the output file
    let output_file = fs_err::File::create(output_path)?;

    // Determine the package name stem (without extension)
    let out_name = format!(
        "{}-{}-{}",
        package.package_record.name.as_normalized(),
        package.package_record.version,
        package.package_record.build
    );

    // Write the conda package
    rattler_package_streaming::write::write_conda_package(
        output_file,
        temp_dir.path(),
        &paths,
        CompressionLevel::Default,
        None, // Use default thread count
        &out_name,
        None, // No specific timestamp
        None, // No progress bar
    )?;

    // Calculate the hash and size of the created package
    let package_bytes = fs_err::read(output_path)?;
    let sha256 = rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(&package_bytes);
    let md5 = rattler_digest::compute_bytes_digest::<rattler_digest::Md5>(&package_bytes);
    let size = package_bytes.len() as u64;

    Ok((sha256, md5, size))
}
