use self::harness::{InstalledDistOptions, MockedSitePackages, NoCache, RequiredPackages};
use crate::NeedReinstall;
use crate::plan::test::harness::AllCached;
use assert_matches::assert_matches;
use harness::empty_wheel;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use url::Url;
use uv_distribution_types::Name;

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
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_eq!(installs.remote.len(), 1);
}

/// Test that we can install a package from the cache when it is available
#[test]
fn test_no_installed_require_one_cached() {
    // No installed packages
    let site_packages = MockedSitePackages::new();
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            AllCached,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert!(installs.remote.is_empty());
    assert_eq!(installs.cached.len(), 1);
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

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    // Should not install package
    assert!(
        installs.reinstalls.is_empty(),
        "found reinstalls: {:?}",
        installs.reinstalls
    );
    assert!(installs.cached.is_empty());
    assert!(installs.remote.is_empty());
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
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::VersionMismatch { ref installed_version, ref locked_version }
        if installed_version.to_string() == "0.6.0" && locked_version.to_string() == "0.7.0"
    );
    assert!(installs.cached.is_empty());
    // Not cached we get it from the remote
    assert_eq!(installs.remote.len(), 1);
}

/// When we have a site-packages with the requested package, and the version does not match we expect
/// a re-installation to occur, with a version mismatch indication
#[test]
fn test_install_required_mismatch_cached() {
    // No installed packages
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.7.0");

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            AllCached,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    // We should install a single package
    // from the remote because we do not cache
    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::VersionMismatch { ref installed_version, ref locked_version }
        if installed_version.to_string() == "0.6.0" && locked_version.to_string() == "0.7.0"
    );
    assert!(installs.remote.is_empty());
    // Not cached we get it from the remote
    assert_eq!(installs.cached.len(), 1);
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
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::InstallerMismatch { ref previous_installer } if previous_installer == "i-am-not-pixi"
    );
    assert!(installs.cached.is_empty());
    // Not cached we get it from the remote
    assert_eq!(installs.remote.len(), 1);
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
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    // We should not do anything
    assert!(installs.cached.is_empty());
    assert!(installs.remote.is_empty());
    assert!(installs.extraneous.is_empty());
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
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::RequiredPythonChanged {
            ref installed_python_require,
            ref locked_python_version
        } if installed_python_require == "<3.12"
        && locked_python_version == "None"
    );
    assert!(installs.cached.is_empty());
    // Not cached we get it from the remote
    assert_eq!(installs.remote.len(), 1);
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
    let required_dists = required.to_required_dists();
    let install_plan = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");
    assert_eq!(install_plan.extraneous.len(), 1);
}

/// Ignored packages should never be marked as extraneous; non-ignored are
/// still removed when unneeded
#[test]
fn test_ignored_packages_not_extraneous() {
    let site_packages = MockedSitePackages::new()
        .add_registry("aiofiles", "0.6.0", InstalledDistOptions::default())
        .add_registry("requests", "2.31.0", InstalledDistOptions::default());
    let required = RequiredPackages::new();

    // Build a planner that ignores `aiofiles` for extraneous detection; `requests`
    // should be considered extraneous and be uninstalled
    let plan = harness::install_planner().with_ignored_extraneous(vec![
        uv_normalize::PackageName::from_str("aiofiles").unwrap(),
    ]);

    let required_dists = required.to_required_dists();
    let install_plan = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should plan");

    // `aiofiles` should not be marked as extraneous; but `requests` should
    let names: Vec<String> = install_plan
        .extraneous
        .iter()
        .map(|d| d.name().to_string())
        .collect();
    assert_eq!(names.len(), 1, "unexpected extraneous: {names:?}");
    assert!(names.contains(&"requests".to_string()));
}

/// When a package was previously installed from a registry, but we now require it from a local source
/// we should reinstall it.
#[test]
fn test_installed_registry_required_local_source() {
    let (temp_dir, _) = harness::fake_pyproject_toml(None);
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default(),
    );
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        temp_dir.path().to_path_buf(),
        false,
    );

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::SourceMismatch { .. }
    );
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
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::UrlMismatch{ ref installed_url, ref locked_url } if *installed_url != locked_url.clone().unwrap()
    );
}

/// When requiring a local package and that same local package is installed, we should not reinstall it
/// except if the source CacheInfo has changed since installation.
#[test]
fn test_installed_local_required_local() {
    let (fake, _) = harness::fake_pyproject_toml(None);

    // Capture the current CacheInfo from the source directory
    let current_cache_info =
        uv_cache_info::CacheInfo::from_path(fake.path()).expect("should get cache info");

    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
        // Provide the same CacheInfo to simulate unchanged source
        InstalledDistOptions::default().with_cache_info(current_cache_info),
    );
    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );

    // Source CacheInfo matches the stored one, so we do not expect a re-installation
    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_eq!(
        installs.reinstalls.len(),
        0,
        "found reinstall: {:?}",
        installs.reinstalls
    );
    assert!(installs.remote.is_empty());
    assert!(installs.cached.is_empty());
}
/// When requiring a local package and that same local package is installed, we should not reinstall it
/// except if the pyproject.toml file, or some other source files we won't check here is newer than the cache
/// NOTE: We are skipping that test since it is flaky on linux
/// uv checks ctime on unix systems
/// During debug, we noticed that some times ctime isn't updated, and we couldn't find a reliable way to ensure that
/// Test that when source is modified after installation, reinstall is triggered.
/// This uses CacheInfo comparison to detect changes.
#[test]
fn test_local_source_newer_than_local_metadata() {
    let (fake, mut pyproject) = harness::fake_pyproject_toml(None);

    // Capture the initial CacheInfo from the source directory
    let initial_cache_info =
        uv_cache_info::CacheInfo::from_path(fake.path()).expect("should get cache info");

    // Modify pyproject.toml to change its content and timestamp
    std::thread::sleep(std::time::Duration::from_millis(10));
    pyproject
        .write_all(b"[build-system]\nrequires = [\"hatchling\"]")
        .unwrap();
    pyproject.sync_all().unwrap();

    // Set up site_packages with the OLD cache info (simulating package installed before modification)
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
        InstalledDistOptions::default().with_cache_info(initial_cache_info),
    );

    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );

    // We expect a reinstall, because the source CacheInfo differs from the stored one
    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");
    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::SourceDirectoryNewerThanCache
    );
}

/// Test that when source CacheInfo matches stored CacheInfo, no reinstall is needed.
#[test]
fn test_local_source_older_than_local_metadata() {
    let (fake, _pyproject) = harness::fake_pyproject_toml(None);

    // Capture the current CacheInfo from the source directory
    let current_cache_info =
        uv_cache_info::CacheInfo::from_path(fake.path()).expect("should get cache info");

    // Set up site_packages with the SAME cache info (simulating source hasn't changed since install)
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
        InstalledDistOptions::default().with_cache_info(current_cache_info),
    );

    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );

    // Install plan should not reinstall anything since CacheInfo matches
    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");
    assert_eq!(installs.reinstalls.len(), 0);
    assert_eq!(installs.cached.len(), 0);
    assert_eq!(installs.remote.len(), 0);
}

/// When we have an editable package installed and we require a non-editable package
/// we should reinstall the non-editable package
#[test]
fn test_installed_editable_required_non_editable() {
    let (fake, _) = harness::fake_pyproject_toml(None);

    // Capture the current CacheInfo from the source directory
    let current_cache_info =
        uv_cache_info::CacheInfo::from_path(fake.path()).expect("should get cache info");

    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        true,
        // Provide matching CacheInfo so freshness check passes
        InstalledDistOptions::default().with_cache_info(current_cache_info),
    );

    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(
        installs.reinstalls[0].1,
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
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(installs.reinstalls[0].1, NeedReinstall::UrlMismatch { .. });

    // If we have the correct archive installed it should not reinstall
    let required = RequiredPackages::new().add_archive("aiofiles", "0.6.0", remote_url.clone());
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");
    assert!(installs.cached.is_empty());
    assert!(installs.remote.is_empty());
}

/// When having a git installed, and we require the same version from the registry
/// we should reinstall, otherwise we should not
/// note that we are using full git commits here, because it seems from my (Tim)
/// testing that these are the ones used in the lock file
#[test]
fn test_installed_git_require_registry() {
    let requested =
        Url::parse("git+https://github.com/pypa/pip.git@9d4f36d87dae9a968fb527e2cb87e8a507b0beb3")
            .expect("could not parse git url");

    let site_packages = MockedSitePackages::new().add_git(
        "pip",
        "1.0.0",
        requested.clone(),
        InstalledDistOptions::default(),
    );
    let required = RequiredPackages::new().add_registry("pip", "1.0.0");

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(installs.reinstalls[0].1, NeedReinstall::UrlMismatch { .. });

    let locked_git_url =
        Url::parse("git+https://github.com/pypa/pip.git?rev=9d4f36d87dae9a968fb527e2cb87e8a507b0beb3#9d4f36d87dae9a968fb527e2cb87e8a507b0beb3")
            .expect("could not parse git url");

    // Okay now we require the same git package, it should not reinstall
    let required = RequiredPackages::new().add_git("pip", "1.0.0", locked_git_url.clone());
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert!(
        installs.reinstalls.is_empty(),
        "found reinstalls: {:?}",
        installs.reinstalls
    );
}

/// When the git commit differs we should reinstall
#[test]
fn test_installed_git_require_git_commit_mismatch() {
    let requested = "9d4f36d";
    let git_url = Url::parse(format!("git+https://github.com/pypa/pip.git@{requested}").as_str())
        .expect("could not parse git url");

    let site_packages = MockedSitePackages::new().add_git(
        "pip",
        "1.0.0",
        git_url.clone(),
        InstalledDistOptions::default(),
    );
    let locked_requested = "cf20850";
    let locked = "cf20850e5e42ba9a71748fdf04193c7857cf5f61";
    let git_url_2 =
        Url::parse(format!("git+https://github.com/pypa/pip.git?rev=cf20850#{locked}").as_str())
            .expect("could not parse git url");
    let required = RequiredPackages::new().add_git("pip", "1.0.0", git_url_2);

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::GitRevMismatch { ref installed_rev, ref locked_rev }
        if requested == installed_rev && locked_requested == locked_rev
    );
}

/// When having a git installed, and we require the same version from the registry
/// we should reinstall, otherwise we should not
/// note that we are using full git commits here, because it seems from my (Tim)
/// testing that these are the ones used in the lock file
#[test]
fn test_installed_git_the_same() {
    let requested = Url::parse("git+https://github.com/pypa/pip.git@some-branch")
        .expect("could not parse git url");

    let site_packages = MockedSitePackages::new().add_git(
        "pip",
        "1.0.0",
        requested.clone(),
        InstalledDistOptions::default(),
    );

    let plan = harness::install_planner();

    let locked_git_url =
        Url::parse("git+https://github.com/pypa/pip.git?branch=some-branch#9d4f36d87dae9a968fb527e2cb87e8a507b0beb3")
            .expect("could not parse git url");

    // Okay now we require the same git package, it should not reinstall
    let required = RequiredPackages::new().add_git("pip", "1.0.0", locked_git_url.clone());
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert!(
        installs.reinstalls.is_empty(),
        "found reinstalls: {:?}",
        installs.reinstalls
    );
}

/// Test that when we refresh a specific package it should reinstall or rebuild
#[test]
fn test_uv_refresh() {
    let site_packages = MockedSitePackages::new().add_registry(
        "aiofiles",
        "0.6.0",
        InstalledDistOptions::default(),
    );
    // Requires following package
    let required = RequiredPackages::new().add_registry("aiofiles", "0.6.0");

    let plan = harness::install_planner();
    let plan = plan.with_uv_refresh(uv_cache::Refresh::from_args(
        Some(true),
        vec![uv_normalize::PackageName::from_str("aiofiles").unwrap()],
    ));
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            AllCached,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    // Should not install package
    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::ReinstallationRequested
    );
    assert!(installs.cached.is_empty());
    assert_eq!(installs.remote.len(), 1);
}

/// Test when we have locked the dependency as a path that we do not re-install when all
/// data is the same.
/// this was a bug that occurred that when having a dependency like
/// ```
/// [pypi-dependencies]
/// foobar = { path = "./foobar-0.1.0-py3-none-any.whl" }
/// ```
/// we would keep reinstalling foobar
#[test]
fn test_archive_is_path() {
    let (tmp, _file, wheel_path) = empty_wheel("foobar-0.1.0-py3-none-any");
    // This needs to be absolute otherwise we cannot parse it into a file url
    let site_packages = MockedSitePackages::new().add_archive(
        "foobar",
        "0.1.0",
        Url::from_file_path(&wheel_path).unwrap(),
        InstalledDistOptions::default(),
    );

    // Requires following package
    let required = RequiredPackages::new().add_local_wheel(
        "foobar",
        "0.1.0",
        PathBuf::from("./some-dir/../foobar-0.1.0-py3-none-any.whl"),
    );
    let plan = harness::install_planner_with_lock_dir(tmp.path().to_path_buf());
    let required_dists = required.to_required_dists_with_lock_dir(tmp.path());
    let installs = plan
        .plan(
            &site_packages,
            AllCached,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");
    // Should not install package
    assert!(installs.reinstalls.is_empty());
    assert!(installs.cached.is_empty());
    assert!(installs.remote.is_empty());
}

#[test]
fn duplicates_are_not_extraneous() {
    let site_packages = MockedSitePackages::new()
        // Our managed package
        .add_registry("aiofiles", "0.6.0", InstalledDistOptions::default())
        // Package not managed by us
        .add_registry(
            "aiofiles",
            "0.6.1",
            InstalledDistOptions::default().with_installer("not-me"),
        );

    // We don't need any package
    let required = RequiredPackages::new();

    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert!(installs.extraneous.is_empty());
    assert_eq!(installs.duplicates.len(), 1);
}

/// Test that custom [tool.uv].cache-keys triggers reinstall when matching files change.
/// This tests the fix for respecting uv's cache-keys configuration.
///
/// Test that custom cache-keys in pyproject.toml are respected.
/// When a file matching the cache-keys pattern is modified, the package should be reinstalled.
#[test]
fn test_custom_cache_keys_trigger_reinstall() {
    // Create temp directory with custom structure
    let temp_dir = tempfile::tempdir().unwrap();

    // Create src directory
    let src_dir = temp_dir.path().join("src");
    fs_err::create_dir_all(&src_dir).unwrap();
    let py_file_path = src_dir.join("mymodule.py");

    // Create pyproject.toml with custom cache-keys
    // Note: cache-keys uses "src/**/*.py" as a string (Path variant)
    {
        let mut pyproject_toml =
            std::fs::File::create(temp_dir.path().join("pyproject.toml")).unwrap();
        pyproject_toml
            .write_all(
                r#"
[build-system]
requires = ["setuptools>=42"]
build-backend = "setuptools.build_meta"

[tool.uv]
cache-keys = ["src/**/*.py"]
"#
                .as_bytes(),
            )
            .unwrap();
        pyproject_toml.sync_all().unwrap();
    }

    // Create the Python file initially
    {
        let mut py_file = std::fs::File::create(&py_file_path).unwrap();
        py_file.write_all(b"# test").unwrap();
        py_file.sync_all().unwrap();
    }

    // Get the initial CacheInfo from the source directory (this is what would be stored at install time)
    let initial_cache_info =
        uv_cache_info::CacheInfo::from_path(temp_dir.path()).expect("should get cache info");

    // Modify the Python file to change its content and timestamp
    std::thread::sleep(std::time::Duration::from_millis(10));
    {
        let mut py_file = std::fs::File::create(&py_file_path).unwrap();
        py_file.write_all(b"# modified test").unwrap();
        py_file.sync_all().unwrap();
    }

    // Set-up site-packages with the OLD cache info (simulating a package installed before modification)
    let site_packages = MockedSitePackages::new().add_directory(
        "testpkg",
        "0.1.0",
        temp_dir.path().to_path_buf(),
        false,
        InstalledDistOptions::default().with_cache_info(initial_cache_info),
    );

    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "testpkg",
        "0.1.0",
        temp_dir.path().to_path_buf(),
        false,
    );

    // We expect a reinstall, because the source CacheInfo differs from the stored one
    let plan = harness::install_planner();
    let required_dists = required.to_required_dists();
    let installs = plan
        .plan(
            &site_packages,
            NoCache,
            &required_dists,
            &uv_configuration::BuildOptions::default(),
        )
        .expect("should install");

    assert_eq!(
        installs.reinstalls.len(),
        1,
        "Expected 1 reinstall but got {}",
        installs.reinstalls.len()
    );
    assert_matches!(
        installs.reinstalls[0].1,
        NeedReinstall::SourceDirectoryNewerThanCache
    );
}
