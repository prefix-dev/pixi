use self::harness::{InstalledDistOptions, MockedSitePackages, NoCache, RequiredPackages};
use crate::install_pypi::NeedReinstall;
use crate::install_pypi::plan::test::harness::AllCached;
use assert_matches::assert_matches;
use harness::empty_wheel;
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
    let plan = harness::install_planner()
        .with_ignored_extraneous(vec![uv_pep508::PackageName::from_str("aiofiles").unwrap()]);

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
    assert_eq!(names.len(), 1, "unexpected extraneous: {:?}", names);
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
/// except if the pyproject.toml file, or some other source files we won't check here is newer than the cache
#[test]
fn test_installed_local_required_local() {
    let ten_minutes_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 10);
    let (fake, _) = harness::fake_pyproject_toml(Some(ten_minutes_ago));
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
/// During debug, we noticed that some times ctime isn't updated, and we couldn't find a relieable way to ensure that
/// At this time, we believe this to be a problem with our test, not with pixi or uv.
/// If user encounter this problem we should investigate this again
#[cfg(not(target_os = "linux"))]
#[test]
fn test_local_source_newer_than_local_metadata() {
    let (fake, pyproject) = harness::fake_pyproject_toml(None);
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
        // Set the metadata mtime to 1 day ago
        InstalledDistOptions::default().with_metadata_mtime(
            std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24),
        ),
    );
    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );
    // Set the pyproject.toml file to be newer than the installed metadata
    // We need to do this otherwise the test seems to fail even though the file should be newer
    pyproject
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(60 * 60 * 24))
        .unwrap();
    pyproject.sync_all().unwrap();

    // We expect a reinstall, because the pyproject.toml file is newer than the cache
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

#[test]
fn test_local_source_older_than_local_metadata() {
    let (fake, pyproject) = harness::fake_pyproject_toml(Some(
        std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24),
    ));
    let site_packages = MockedSitePackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
        // Set the metadata mtime to now explicitly
        InstalledDistOptions::default().with_metadata_mtime(std::time::SystemTime::now()),
    );
    // Requires following package
    let required = RequiredPackages::new().add_directory(
        "aiofiles",
        "0.6.0",
        fake.path().to_path_buf(),
        false,
    );

    let dist_info = site_packages
        .base_dir()
        .join(format!("{}-{}.dist-info", "aiofiles", "0.6.0"))
        .join("METADATA");
    // Sanity check that these timestamps are different
    assert_ne!(
        pyproject.metadata().unwrap().modified().unwrap(),
        dist_info.metadata().unwrap().modified().unwrap()
    );

    // Install plan should not reinstall anything
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
        vec![uv_pep508::PackageName::from_str("aiofiles").unwrap()],
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
