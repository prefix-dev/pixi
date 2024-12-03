#[cfg(test)]
use rattler_lock::{PypiPackageData, UrlOrPath};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    str::FromStr,
};
use tempfile::TempDir;
use typed_path::Utf8TypedPathBuf;
use uv_pypi_types::DirectUrl;

use pixi_consts::consts;
use url::Url;
use uv_distribution_types::{InstalledDirectUrlDist, InstalledDist, InstalledRegistryDist};

use crate::install_pypi::{
    plan::{CachedDistProvider, InstallPlanner, InstalledDistProvider},
    NeedReinstall,
};

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
}

#[derive(Default)]
/// Some configuration options for the installed dist
struct InstalledDistOptions {
    installer: Option<String>,
    requires_python: Option<uv_pep440::VersionSpecifiers>,
}

impl InstalledDistOptions {
    fn with_installer<S: AsRef<str>>(mut self, installer: S) -> Self {
        self.installer = Some(installer.as_ref().to_owned());
        self
    }

    fn with_requires_python<S: AsRef<str>>(mut self, requires_python: S) -> Self {
        self.requires_python =
            uv_pep440::VersionSpecifiers::from_str(requires_python.as_ref()).ok();
        self
    }

    fn installer(&self) -> &str {
        self.installer
            .as_deref()
            .unwrap_or(consts::PIXI_UV_INSTALLER)
    }

    fn requires_python(&self) -> Option<&uv_pep440::VersionSpecifiers> {
        self.requires_python.as_ref()
    }
}

struct MockedSitePackages {
    installed_dist: Vec<InstalledDist>,
    /// This is the fake site packages directory, we need a file-backing for some of the
    /// re-installation checks
    fake_site_packages: tempfile::TempDir,
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
        std::fs::create_dir_all(&dist_info).expect("should create dist-info");

        // Write the INSTALLER file
        let installer = opts.installer();
        std::fs::write(dist_info.join("INSTALLER"), installer).expect("could not write INSTALLER");

        // Write the METADATA file
        let raw_metadata = "Name: {name}\nVersion: {version}\nSummary: A test package";
        let mut minimal_metadata = raw_metadata
            .replace("{name}", name)
            .replace("{version}", version);
        if let Some(requires_python) = opts.requires_python() {
            let requires_python = format!("\nRequires-Python: {}", requires_python);
            minimal_metadata.push_str(&requires_python);
        }
        println!("metadata: {}", minimal_metadata);
        std::fs::write(dist_info.join("METADATA"), minimal_metadata)
            .expect("could not write METADATA");
        dist_info
    }

    /// Create a direct url for the installed dist
    fn create_direct_url(&self, dist_info: &Path, direct_url: DirectUrl) {
        let json = serde_json::to_string(&direct_url).expect("should serialize");
        let direct_url = dist_info.join("direct_url.json");
        std::fs::write(&direct_url, json).expect("should write direct url");
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
        rattler_lock::PypiPackageData {
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
        rattler_lock::PypiPackageData {
            name: pep508_rs::PackageName::new(name.as_ref().to_owned()).unwrap(),
            version: pep440_rs::Version::from_str(version.as_ref()).unwrap(),
            // We dont check these fields, for determining the installation from a registry
            //
            requires_dist: vec![],
            requires_python: None,
            location: UrlOrPath::Path(Utf8TypedPathBuf::from(path.to_string_lossy().to_string())),
            hash: None,
            editable,
        }
    }
}

/// Implementor of the [`CachedDistProvider`] that does not cache anything
struct NoCache;
impl<'a> CachedDistProvider<'a> for NoCache {
    fn get_cached_dist(
        &mut self,
        _name: &'a uv_normalize::PackageName,
        _version: uv_pep440::Version,
    ) -> Option<uv_distribution_types::CachedRegistryDist> {
        None
    }
}

/// Struct to create the required packages map
#[derive(Default)]
struct RequiredPackages {
    required: HashMap<uv_normalize::PackageName, PypiPackageData>,
}

impl RequiredPackages {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a registry package to the required packages
    fn add_registry<S: AsRef<str>>(mut self, name: S, version: S) -> Self {
        let package_name =
            uv_normalize::PackageName::new(name.as_ref().to_owned()).expect("should be correct");
        let data = PyPIPackageDataBuilder::registry(name, version);
        self.required.insert(package_name, data);
        self
    }

    /// Add a directory package to the required packages
    fn add_directory<S: AsRef<str>>(
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

    /// Convert the required packages where it the data is borrowed
    /// this is needed to pass it into the [`InstallPlanner`]
    fn to_borrowed(&self) -> HashMap<uv_normalize::PackageName, &PypiPackageData> {
        self.required.iter().map(|(k, v)| (k.clone(), v)).collect()
    }
}

/// Python version to use throughout the tests
const TEST_PYTHON_VERSION: &str = "3.12";
/// Some python version
fn python_version() -> uv_pep440::Version {
    uv_pep440::Version::from_str(TEST_PYTHON_VERSION).unwrap()
}

/// Simple function to create an install planner
fn install_planner() -> InstallPlanner {
    InstallPlanner::new(
        uv_cache::Cache::temp().unwrap(),
        &python_version(),
        PathBuf::new(),
    )
}

/// Create a fake pyproject.toml file in a temp dir
/// return the temp dir
fn fake_pyproject_toml(
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

/// When no site-packages exist and we have requested an uncached package
/// we expect an install from the remote
#[test]
fn test_no_installed_require_one() {
    // No installed packages
    let site_packages = MockedSitePackages::new();
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_eq!(install_plan.remote.len(), 1);
}

/// When we have a site-packages with the requested package, and the version matches we expect
/// no re-installation to occur
#[test]
fn test_install_required_equivalent() {
    // No installed packages
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_eq!(
        install_plan.reinstalls.len(),
        0,
        "found reinstalls: {:?}",
        install_plan.reinstalls
    );
    assert_eq!(install_plan.local.len(), 0);
    assert_eq!(install_plan.remote.len(), 0);
}

/// When we have a site-packages with the requested package, and the version does not match we expect
/// a re-installation to occur, with a version mismatch
#[test]
fn test_install_required_mismatch() {
    // No installed packages
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.7.0");

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_matches::assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::VersionMismatch { ref installed_version, ref locked_version }
        if installed_version.to_string() == "0.6.0" && locked_version.to_string() == "0.7.0"
    );
    assert_eq!(install_plan.local.len(), 0);
    // Not cached we get it from the remote
    assert_eq!(install_plan.remote.len(), 1);
}

/// When requiring a package that has a different INSTALLER but we *do require* it
/// we should reinstall it
#[test]
fn test_install_required_installer_mismatch() {
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default().with_installer("i-am-not-pixi"),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches::assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::InstallerMismatch { ref previous_installer } if previous_installer == "i-am-not-pixi"
    );
    assert_eq!(install_plan.local.len(), 0);
    // Not cached we get it from the remote
    assert_eq!(install_plan.remote.len(), 1);
}

/// When having a package with a different INSTALLER and we do not require it, we should leave it alone
/// and not reinstall it
#[test]
fn test_installed_one_other_installer() {
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default().with_installer("i-am-not-pixi"),
    );
    // Nothing is required
    let required = RequiredPackages::new();

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // We should not do anything
    assert_eq!(install_plan.local.len(), 0);
    assert_eq!(install_plan.remote.len(), 0);
}

/// When requiring a package that has a different required python then we have installed we want to reinstall
/// the package
#[test]
fn test_install_required_python_mismatch() {
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default().with_requires_python("<3.12"),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches::assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::RequiredPythonChanged {
            ref installed_python_require,
            ref locked_python_version
        } if installed_python_require.to_string() == "<3.12"
        && locked_python_version.to_string() == TEST_PYTHON_VERSION
    );
    assert_eq!(install_plan.local.len(), 0);
    // Not cached we get it from the remote
    assert_eq!(install_plan.remote.len(), 1);
}

/// When no longer requiring a package that is installed we should uninstall it,
/// i.e. mark as extraneous
#[test]
fn test_installed_one_none_required() {
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default(),
    );
    let required = RequiredPackages::new();

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");
    assert_eq!(install_plan.extraneous.len(), 1);
}

/// When requiring a package from the registry that is currently installed as a directory
/// it should be re-installed
#[test]
fn test_installed_local_required_registry() {
    let (temp_dir, _) = fake_pyproject_toml(None);
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        temp_dir.path().to_path_buf(),
        false,
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches::assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::UrlMismatch{ ref installed_url, ref locked_url } if *installed_url != locked_url.clone().unwrap()
    );
}

/// When requiring a local package and that same local package is installed, we should not reinstall it
/// except if the pyproject.toml file, or some other source files we wont check here is newer than the cache
#[test]
fn test_installed_local_required_local() {
    let ten_minutes_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 10);
    let (fake, pyproject_toml) = fake_pyproject_toml(Some(ten_minutes_ago));
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );

    // Pyproject.toml file is older than the cache, all else is the same
    // so we do not expect a reinstall
    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_eq!(
        install_plan.reinstalls.len(),
        0,
        "found reinstalls: {:?}",
        install_plan.reinstalls
    );
    assert_eq!(install_plan.remote.len(), 0);
    assert_eq!(install_plan.local.len(), 0);

    // Lets update the pyproject.toml mtime, then we do expect a reinstall
    pyproject_toml
        .set_modified(std::time::SystemTime::now())
        .unwrap();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");
    assert_matches::assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::SourceDirectoryNewerThanCache
    );
}

/// When we have an editable package installed and we require a non-editable package
/// we should reinstall the editable package
#[test]
fn test_installed_editable_required_non_editable() {
    let (fake, _) = fake_pyproject_toml(None);
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        true,
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );

    let plan = install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches::assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::EditableStatusChanged {
            locked_editable: false,
            installed_editable: true
        }
    );
}
