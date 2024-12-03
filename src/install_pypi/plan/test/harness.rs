use crate::install_pypi::plan::{CachedDistProvider, InstallPlanner, InstalledDistProvider};
use pixi_consts::consts;
use pixi_manifest::pypi::pypi_requirement::ParsedGitUrl;
use rattler_lock::{PypiPackageData, UrlOrPath};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tempfile::TempDir;
use typed_path::Utf8TypedPathBuf;
use url::Url;
use uv_distribution_filename::WheelFilename;
use uv_distribution_types::{InstalledDirectUrlDist, InstalledDist, InstalledRegistryDist};
use uv_pypi_types::DirectUrl::VcsUrl;
use uv_pypi_types::{ArchiveInfo, DirectUrl, VcsInfo, VcsKind};

#[derive(Default)]
/// Builder to create installed dists
struct InstalledDistBuilder;

impl InstalledDistBuilder {
    pub fn registry<S: AsRef<str>>(name: S, version: S, path: PathBuf) -> InstalledDist {
        let name =
            uv_pep508::PackageName::new(name.as_ref().to_owned()).expect("unable to normalize");
        let version =
            uv_pep440::Version::from_str(version.as_ref()).expect("cannot parse pep440 version");

        let registry = InstalledRegistryDist {
            name,
            version,
            path,
            cache_info: None,
        };
        InstalledDist::Registry(registry)
    }

    pub fn directory<S: AsRef<str>>(
        name: S,
        version: S,
        install_path: PathBuf,
        source_path: PathBuf,
        editable: bool,
    ) -> (InstalledDist, DirectUrl) {
        let name =
            uv_pep508::PackageName::new(name.as_ref().to_owned()).expect("unable to normalize");
        let version =
            uv_pep440::Version::from_str(version.as_ref()).expect("cannot parse pep440 version");
        let directory_url = Url::from_file_path(&source_path).unwrap();

        let direct_url = DirectUrl::LocalDirectory {
            url: directory_url.to_string(),
            dir_info: uv_pypi_types::DirInfo {
                editable: Some(editable),
            },
        };

        let installed_direct_url = InstalledDirectUrlDist {
            name,
            version,
            direct_url: Box::new(direct_url.clone()),
            url: directory_url,
            editable,
            path: install_path,
            cache_info: None,
        };
        (InstalledDist::Url(installed_direct_url), direct_url)
    }

    pub fn archive<S: AsRef<str>>(
        name: S,
        version: S,
        install_path: PathBuf,
        url: Url,
    ) -> (InstalledDist, DirectUrl) {
        let name =
            uv_pep508::PackageName::new(name.as_ref().to_owned()).expect("unable to normalize");
        let version =
            uv_pep440::Version::from_str(version.as_ref()).expect("cannot parse pep440 version");

        let direct_url = DirectUrl::ArchiveUrl {
            url: url.to_string(),
            archive_info: ArchiveInfo {
                hashes: None,
                hash: None,
            },
            subdirectory: None,
        };

        let installed_direct_url = InstalledDirectUrlDist {
            name,
            version,
            direct_url: Box::new(direct_url.clone()),
            url,
            editable: false,
            path: install_path,
            cache_info: None,
        };
        (InstalledDist::Url(installed_direct_url), direct_url)
    }

    pub fn git<S: AsRef<str>>(
        name: S,
        version: S,
        install_path: PathBuf,
        url: Url,
    ) -> (InstalledDist, DirectUrl) {
        let name =
            uv_pep508::PackageName::new(name.as_ref().to_owned()).expect("unable to normalize");
        let version =
            uv_pep440::Version::from_str(version.as_ref()).expect("cannot parse pep440 version");

        // Parse git url and extract git commit, use this as the commit_id
        let parsed_git_url = ParsedGitUrl::try_from(url.clone()).expect("should parse git url");

        let direct_url = VcsUrl {
            url: url.to_string(),
            subdirectory: None,
            vcs_info: VcsInfo {
                vcs: VcsKind::Git,
                commit_id: parsed_git_url.rev.map(|r| r.to_string()),
                requested_revision: None,
            },
        };

        let installed_direct_url = InstalledDirectUrlDist {
            name,
            version,
            direct_url: Box::new(direct_url.clone()),
            url,
            path: install_path,
            editable: false,
            cache_info: None,
        };
        (InstalledDist::Url(installed_direct_url), direct_url)
    }
}

#[derive(Default)]
/// Some configuration options for the installed dist
pub struct InstalledDistOptions {
    installer: Option<String>,
    requires_python: Option<uv_pep440::VersionSpecifiers>,
    metadata_mtime: Option<std::time::SystemTime>,
}

impl InstalledDistOptions {
    pub fn with_installer<S: AsRef<str>>(mut self, installer: S) -> Self {
        self.installer = Some(installer.as_ref().to_owned());
        self
    }

    pub fn with_requires_python<S: AsRef<str>>(mut self, requires_python: S) -> Self {
        self.requires_python =
            uv_pep440::VersionSpecifiers::from_str(requires_python.as_ref()).ok();
        self
    }

    pub fn with_metadata_mtime(mut self, metadata_mtime: std::time::SystemTime) -> Self {
        self.metadata_mtime = Some(metadata_mtime);
        self
    }

    pub fn installer(&self) -> &str {
        self.installer
            .as_deref()
            .unwrap_or(consts::PIXI_UV_INSTALLER)
    }

    pub fn requires_python(&self) -> Option<&uv_pep440::VersionSpecifiers> {
        self.requires_python.as_ref()
    }

    pub fn metadata_mtime(&self) -> Option<std::time::SystemTime> {
        self.metadata_mtime
    }
}

pub struct MockedSitePackages {
    installed_dist: Vec<InstalledDist>,
    /// This is the fake site packages directory, we need a file-backing for some of the
    /// re-installation checks
    fake_site_packages: TempDir,
}

impl MockedSitePackages {
    pub fn new() -> Self {
        Self {
            installed_dist: vec![],
            fake_site_packages: tempfile::tempdir().expect("should create temp dir"),
        }
    }

    /// Create INSTALLER and METADATA files for the installed dist
    /// these are checked for the installer and requires python
    fn create_file_backing(
        &self,
        name: &str,
        version: &str,
        opts: InstalledDistOptions,
    ) -> PathBuf {
        // Create the dist-info directory
        let dist_info = format!("{}-{}.dist-info", name, version);
        let dist_info = self.fake_site_packages.path().join(dist_info);
        fs_err::create_dir_all(&dist_info).expect("should create dist-info");

        // Write the INSTALLER file
        let installer = opts.installer();
        fs_err::write(dist_info.join("INSTALLER"), installer).expect("could not write INSTALLER");

        // Write the METADATA file
        let raw_metadata = "Name: {name}\nVersion: {version}\nSummary: A test package";
        let mut minimal_metadata = raw_metadata
            .replace("{name}", name)
            .replace("{version}", version);
        if let Some(requires_python) = opts.requires_python() {
            let requires_python = format!("\nRequires-Python: {}", requires_python);
            minimal_metadata.push_str(&requires_python);
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .truncate(true)
            .open(dist_info.join("METADATA"))
            .unwrap();
        file.write_all(minimal_metadata.as_bytes())
            .expect("should write metadata");

        if let Some(metadata_mtime) = opts.metadata_mtime() {
            file.set_modified(metadata_mtime)
                .expect("should set modified time");
            file.sync_all().expect("should sync file");
        }

        dist_info
    }

    /// Create a direct url for the installed dist
    fn create_direct_url(&self, dist_info: &Path, direct_url: DirectUrl) {
        let json = serde_json::to_string(&direct_url).expect("should serialize");
        let direct_url = dist_info.join("direct_url.json");
        fs_err::write(&direct_url, json).expect("should write direct url");
    }

    /// Add a registry installed dist to the site packages
    pub fn add_registry<S: AsRef<str>>(
        mut self,
        name: S,
        version: S,
        opts: InstalledDistOptions,
    ) -> Self {
        let dist_info = self.create_file_backing(name.as_ref(), version.as_ref(), opts);
        self.installed_dist
            .push(InstalledDistBuilder::registry(name, version, dist_info));
        self
    }

    /// Add a local directory that serves as an installed dist to the site-packages
    pub fn add_directory<S: AsRef<str>>(
        mut self,
        name: S,
        version: S,
        source_path: PathBuf,
        editable: bool,
        opts: InstalledDistOptions,
    ) -> Self {
        let dist_info = self.create_file_backing(name.as_ref(), version.as_ref(), opts);
        let (installed_dist, direct_url) = InstalledDistBuilder::directory(
            name,
            version,
            dist_info.clone(),
            source_path,
            editable,
        );
        self.create_direct_url(&dist_info, direct_url);
        self.installed_dist.push(installed_dist);
        self
    }

    /// Add an archive installed dist to the site packages
    pub fn add_archive<S: AsRef<str>>(
        mut self,
        name: S,
        version: S,
        url: Url,
        opts: InstalledDistOptions,
    ) -> Self {
        let dist_info = self.create_file_backing(name.as_ref(), version.as_ref(), opts);
        let (installed_dist, direct_url) =
            InstalledDistBuilder::archive(name, version, dist_info.clone(), url);
        self.create_direct_url(&dist_info, direct_url);
        self.installed_dist.push(installed_dist);
        self
    }

    /// Add a git installed dist to the site packages
    pub fn add_git<S: AsRef<str>>(
        mut self,
        name: S,
        version: S,
        url: Url,
        opts: InstalledDistOptions,
    ) -> Self {
        let dist_info = self.create_file_backing(name.as_ref(), version.as_ref(), opts);
        let (installed_dist, direct_url) =
            InstalledDistBuilder::git(name, version, dist_info.clone(), url);
        self.create_direct_url(&dist_info, direct_url);
        self.installed_dist.push(installed_dist);
        self
    }
}

impl<'a> InstalledDistProvider<'a> for MockedSitePackages {
    fn iter(&'a self) -> impl Iterator<Item = &'a InstalledDist> {
        self.installed_dist.iter()
    }
}

#[derive(Default)]
/// Builder to create pypi package data, this is essentially the locked data
struct PyPIPackageDataBuilder;

impl PyPIPackageDataBuilder {
    fn registry<S: AsRef<str>>(name: S, version: S) -> PypiPackageData {
        PypiPackageData {
            name: pep508_rs::PackageName::new(name.as_ref().to_owned()).unwrap(),
            version: pep440_rs::Version::from_str(version.as_ref()).unwrap(),
            // We dont check these fields, for determining the installation from a registry
            //
            requires_dist: vec![],
            requires_python: None,
            location: UrlOrPath::Url(
                Url::parse(&format!(
                    "https://pypi.org/{name}-{version}-py3-none-any.whl",
                    name = name.as_ref(),
                    version = version.as_ref()
                ))
                .unwrap(),
            ),
            hash: None,
            editable: false,
        }
    }

    fn directory<S: AsRef<str>>(
        name: S,
        version: S,
        path: PathBuf,
        editable: bool,
    ) -> PypiPackageData {
        PypiPackageData {
            name: pep508_rs::PackageName::new(name.as_ref().to_owned()).unwrap(),
            version: pep440_rs::Version::from_str(version.as_ref()).unwrap(),
            requires_dist: vec![],
            requires_python: None,
            location: UrlOrPath::Path(Utf8TypedPathBuf::from(path.to_string_lossy().to_string())),
            hash: None,
            editable,
        }
    }

    fn direct_url<S: AsRef<str>>(name: S, version: S, url: Url) -> PypiPackageData {
        // Create new url with direct+ in the scheme
        let url = Url::parse(&format!("direct+{}", url)).unwrap();
        PypiPackageData {
            name: pep508_rs::PackageName::new(name.as_ref().to_owned()).unwrap(),
            version: pep440_rs::Version::from_str(version.as_ref()).unwrap(),
            requires_dist: vec![],
            requires_python: None,
            location: UrlOrPath::Url(url),
            hash: None,
            editable: false,
        }
    }

    fn git<S: AsRef<str>>(name: S, version: S, url: Url) -> PypiPackageData {
        PypiPackageData {
            name: pep508_rs::PackageName::new(name.as_ref().to_owned()).unwrap(),
            version: pep440_rs::Version::from_str(version.as_ref()).unwrap(),
            requires_dist: vec![],
            requires_python: None,
            location: UrlOrPath::Url(url),
            hash: None,
            editable: false,
        }
    }
}

/// Implementor of the [`CachedDistProvider`] that does not cache anything
pub struct NoCache;

impl<'a> CachedDistProvider<'a> for NoCache {
    fn get_cached_dist(
        &mut self,
        _name: &'a uv_normalize::PackageName,
        _version: uv_pep440::Version,
    ) -> Option<uv_distribution_types::CachedRegistryDist> {
        None
    }
}

/// Implementor of the [`CachedDistProvider`] that assumes to have cached everything
pub struct AllCached;
impl<'a> CachedDistProvider<'a> for AllCached {
    fn get_cached_dist(
        &mut self,
        name: &'a uv_normalize::PackageName,
        version: uv_pep440::Version,
    ) -> Option<uv_distribution_types::CachedRegistryDist> {
        let wheel_filename =
            WheelFilename::from_str(format!("{}-{}-py3-none-any.whl", name, version).as_str())
                .unwrap();
        let dist = uv_distribution_types::CachedRegistryDist {
            filename: wheel_filename,
            path: Default::default(),
            hashes: vec![],
            cache_info: Default::default(),
        };
        Some(dist)
    }
}

/// Struct to create the required packages map
#[derive(Default)]
pub struct RequiredPackages {
    required: HashMap<uv_normalize::PackageName, PypiPackageData>,
}

impl RequiredPackages {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a registry package to the required packages
    pub fn add_registry<S: AsRef<str>>(mut self, name: S, version: S) -> Self {
        let package_name =
            uv_normalize::PackageName::new(name.as_ref().to_owned()).expect("should be correct");
        let data = PyPIPackageDataBuilder::registry(name, version);
        self.required.insert(package_name, data);
        self
    }

    /// Add a directory package to the required packages
    pub fn add_directory<S: AsRef<str>>(
        mut self,
        name: S,
        version: S,
        path: PathBuf,
        editable: bool,
    ) -> Self {
        let package_name =
            uv_normalize::PackageName::new(name.as_ref().to_owned()).expect("should be correct");
        let data = PyPIPackageDataBuilder::directory(name, version, path, editable);
        self.required.insert(package_name, data);
        self
    }

    pub fn add_archive<S: AsRef<str>>(mut self, name: S, version: S, url: Url) -> Self {
        let package_name =
            uv_normalize::PackageName::new(name.as_ref().to_owned()).expect("should be correct");
        let data = PyPIPackageDataBuilder::direct_url(name, version, url);
        self.required.insert(package_name, data);
        self
    }

    pub fn add_git<S: AsRef<str>>(mut self, name: S, version: S, url: Url) -> Self {
        let package_name =
            uv_normalize::PackageName::new(name.as_ref().to_owned()).expect("should be correct");
        let data = PyPIPackageDataBuilder::git(name, version, url);
        self.required.insert(package_name, data);
        self
    }

    /// Convert the required packages where it the data is borrowed
    /// this is needed to pass it into the [`InstallPlanner`]
    pub fn to_borrowed(&self) -> HashMap<uv_normalize::PackageName, &PypiPackageData> {
        self.required.iter().map(|(k, v)| (k.clone(), v)).collect()
    }
}

/// Python version to use throughout the tests
pub const TEST_PYTHON_VERSION: &str = "3.12";

/// Some python version
fn python_version() -> uv_pep440::Version {
    uv_pep440::Version::from_str(TEST_PYTHON_VERSION).unwrap()
}

/// Simple function to create an install planner
pub fn install_planner() -> InstallPlanner {
    InstallPlanner::new(
        uv_cache::Cache::temp().unwrap(),
        &python_version(),
        PathBuf::new(),
    )
}

/// Create a fake pyproject.toml file in a temp dir
/// return the temp dir
pub fn fake_pyproject_toml(
    modification_time: Option<std::time::SystemTime>,
) -> (TempDir, std::fs::File) {
    let temp_dir = tempfile::tempdir().unwrap();
    let pyproject_toml = temp_dir.path().join("pyproject.toml");
    let mut pyproject_toml = std::fs::File::create(pyproject_toml).unwrap();
    pyproject_toml
        .write_all(
            r#"
            [build-system]
            requires = ["setuptools>=42"]
            build-backend = "setuptools.build_meta"
            "#
            .as_bytes(),
        )
        .unwrap();
    // Set the modification time if it is provided
    if let Some(modification_time) = modification_time {
        pyproject_toml.set_modified(modification_time).unwrap();
    }
    (temp_dir, pyproject_toml)
}
