use self::harness::{InstalledDistOptions, MockedSitePackages, NoCache, RequiredPackages};
use crate::install_pypi::plan::test::harness::TEST_PYTHON_VERSION;
use crate::install_pypi::NeedReinstall;
use assert_matches::assert_matches;
use url::Url;

mod harness;

/// When no site-packages exist, and we have requested an uncached package
/// we expect an installation from the remote
#[test]
fn test_no_installed_require_one() {
    // No installed packages
    let site_packages = MockedSitePackages::new();
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_eq!(install_plan.remote.len(), 1);
}

// Test that we can install a package from the cache when it is available

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

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // Should not install package
    assert!(
        install_plan.reinstalls.is_empty(),
        "found reinstalls: {:?}",
        install_plan.reinstalls
    );
    assert!(install_plan.local.is_empty());
    assert!(install_plan.remote.is_empty());
}

/// When we have a site-packages with the requested package, and the version does not match we expect
/// a re-installation to occur, with a version mismatch indication
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

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::VersionMismatch { ref installed_version, ref locked_version }
        if installed_version.to_string() == "0.6.0" && locked_version.to_string() == "0.7.0"
    );
    assert!(install_plan.local.is_empty());
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

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::InstallerMismatch { ref previous_installer } if previous_installer == "i-am-not-pixi"
    );
    assert!(install_plan.local.is_empty());
    // Not cached we get it from the remote
    assert_eq!(install_plan.remote.len(), 1);
}

/// When having a package with a different INSTALLER, and we do not require it, we should leave it alone
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

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    // We should not do anything
    assert!(install_plan.local.is_empty());
    assert!(install_plan.remote.is_empty());
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

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::RequiredPythonChanged {
            ref installed_python_require,
            ref locked_python_version
        } if installed_python_require.to_string() == "<3.12"
        && locked_python_version.to_string() == TEST_PYTHON_VERSION
    );
    assert!(install_plan.local.is_empty());
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

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");
    assert_eq!(install_plan.extraneous.len(), 1);
}

/// When requiring a package from the registry that is currently installed as a directory
/// it should be re-installed
#[test]
fn test_installed_local_required_registry() {
    let (temp_dir, _) = harness::fake_pyproject_toml(None);
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        temp_dir.path().to_path_buf(),
        false,
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::UrlMismatch{ ref installed_url, ref locked_url } if *installed_url != locked_url.clone().unwrap()
    );
}

/// When requiring a local package and that same local package is installed, we should not reinstall it
/// except if the pyproject.toml file, or some other source files we won't check here is newer than the cache
#[test]
fn test_installed_local_required_local() {
    let ten_minutes_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 10);
    let (fake, pyproject_toml) = harness::fake_pyproject_toml(Some(ten_minutes_ago));
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

    // pyproject.toml file is older than the cache, all else is the same
    // so we do not expect a re-installation
    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_eq!(
        install_plan.reinstalls.len(),
        0,
        "found reinstall: {:?}",
        install_plan.reinstalls
    );
    assert!(install_plan.remote.is_empty());
    assert!(install_plan.local.is_empty());

    // Let's update the pyproject.toml mtime, then we do expect a re-installation
    pyproject_toml
        .set_modified(std::time::SystemTime::now())
        .unwrap();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");
    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::SourceDirectoryNewerThanCache
    );
}

/// When we have an editable package installed and we require a non-editable package
/// we should reinstall the non-editable package
#[test]
fn test_installed_editable_required_non_editable() {
    let (fake, _) = harness::fake_pyproject_toml(None);
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

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::EditableStatusChanged {
            locked_editable: false,
            installed_editable: true
        }
    );
}

/// When having a direct archive installed and we require the same version from the registry
/// we should reinstall
#[test]
fn test_installed_archive_require_registry() {
    let remote_url =
        Url::parse("https://some-other-registry.org/aiofiles-0.6.0-py3-none-any.whl").unwrap();
    let site_packages = MockedSitePackages::new().add_archive(
        "aiofiles",
        "0.6.0",
        remote_url.clone(),
        InstalledDistOptions::default(),
    );

    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");
    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::UrlMismatch { .. }
    );

    // If we have the correct archive installed it should not reinstall
    let required = RequiredPackages::new().add_archive("aiofiles", "0.6.0", remote_url.clone());
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");
    assert!(install_plan.local.is_empty());
    assert!(install_plan.remote.is_empty());
}

/// When having a git installed, and we require the same version from the registry
/// we should reinstall, otherwise we should not
/// note that we are using full git commits here, because it seems from my (Tim)
/// testing that these are the ones used in the lock file
#[test]
fn test_installed_git_require_registry() {
    let git_url =
        Url::parse("git+https://github.com/pypa/pip.git@9d4f36d87dae9a968fb527e2cb87e8a507b0beb3")
            .expect("could not parse git url");

    let site_packages = MockedSitePackages::new().add_git(
        "pip",
        "1.0.0",
        git_url.clone(),
        InstalledDistOptions::default(),
    );
    let required = RequiredPackages::new().add_registry("pip", "1.0.0");

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");
    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::UrlMismatch { .. }
    );

    // Okay now we require the same git package, it should not reinstall
    let required = RequiredPackages::new().add_git("pip", "1.0.0", git_url.clone());
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");
    assert!(
        install_plan.reinstalls.is_empty(),
        "found reinstalls: {:?}",
        install_plan.reinstalls
    );
}

/// When the git commit differs we should reinstall
#[test]
fn test_installed_git_require_git_commit_mismatch() {
    let installed = "9d4f36d87dae9a968fb527e2cb87e8a507b0beb3";
    let git_url = Url::parse(format!("git+https://github.com/pypa/pip.git@{installed}").as_str())
        .expect("could not parse git url");

    let site_packages = MockedSitePackages::new().add_git(
        "pip",
        "1.0.0",
        git_url.clone(),
        InstalledDistOptions::default(),
    );
    let locked = "cf20850e5e42ba9a71748fdf04193c7857cf5f61";
    let git_url_2 = Url::parse(format!("git+https://github.com/pypa/pip.git@{locked}").as_str())
        .expect("could not parse git url");
    let required = RequiredPackages::new().add_git("pip", "1.0.0", git_url_2);

    let plan = harness::install_planner();
    let install_plan = plan
        .plan(&site_packages, NoCache, &required.to_borrowed())
        .expect("should install");

    assert_matches!(
        install_plan.reinstalls[0].1,
        NeedReinstall::GitCommitsMismatch { ref installed_commit, ref locked_commit }
        if installed == installed_commit && locked == locked_commit
    );
}
