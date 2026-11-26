use pixi_test_utils::{MockRepoData, Package};
use rattler_conda_types::Subdir;

use crate::{
    common::{
        LockFileExt, PixiControl,
        pypi_index::{Database as PyPIDatabase, PyPIPackage},
    },
    setup_tracing,
};

#[tokio::test]
async fn temporary_constraints_intersect_across_environments() {
    setup_tracing();
    let platform = Subdir::current().unwrap_or(Subdir::NoArch);
    let mut packages = MockRepoData::default();
    for version in ["3.11.0", "3.12.0"] {
        packages.add_package(
            Package::build("python", version)
                .with_subdir(platform)
                .finish(),
        );
    }
    let channel = packages.into_channel().await.unwrap();
    let index = PyPIDatabase::new()
        .with(PyPIPackage::new("consumer", "1.0.0").with_requires_dist(["foo-bar>=1"]))
        .with(PyPIPackage::new("foo-bar", "1.0.0"))
        .with(PyPIPackage::new("foo-bar", "2.0.0"))
        .with(PyPIPackage::new("foo-bar", "3.0.0"))
        .into_simple_index()
        .unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel}"]
        platforms = ["{platform}"]
        conda-pypi-map = false
        [dependencies]
        python = "==3.12.0"
        [pypi-dependencies]
        consumer = "*"
        [pypi-constraints]
        foo-bar = ">=2"
        [pypi-options]
        index-url = "{index}"
        [feature.older.dependencies]
        python = "==3.11.0"
        [feature.older.pypi-dependencies]
        consumer = "*"
        [feature.older.pypi-options]
        index-url = "{index}"
        [environments]
        older = {{ features = ["older"], no-default-feature = true }}
        "#,
        channel = channel.url(),
        index = index.index_url(),
    ))
    .unwrap();
    pixi.lock().await.unwrap();
    let manifest_before = pixi.manifest_contents().unwrap();

    pixi.lock()
        .with_pypi_constraint("Foo_Bar>=2")
        .with_pypi_constraint("foo.bar<3")
        .with_pypi_constraint("absent-package>=999")
        .await
        .unwrap();
    let lock = pixi.lock_file().await.unwrap();
    for environment in ["default", "older"] {
        assert_eq!(
            lock.get_pypi_package_version(environment, platform, "foo-bar"),
            Some("2.0.0".into())
        );
        assert!(!lock.contains_pypi_package(environment, platform, "absent-package"));
    }
    let lock_path = pixi.workspace_path().join("pixi.lock");
    let before = fs_err::read(&lock_path).unwrap();

    // Both repeated CLI bounds and manifest bounds must remain in force.
    for constraints in [["foo-bar>=3", "foo-bar<3"], ["foo-bar<2", "foo-bar>=1"]] {
        pixi.lock()
            .with_pypi_constraint(constraints[0])
            .with_pypi_constraint(constraints[1])
            .await
            .expect_err("incompatible bounds must not replace one another");
        assert_eq!(fs_err::read(&lock_path).unwrap(), before);
        assert_eq!(pixi.manifest_contents().unwrap(), manifest_before);
    }

    pixi.lock()
        .with_pypi_constraint("foo-bar>=3; python_version < '3.12'")
        .await
        .unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert_eq!(
        lock.get_pypi_package_version("default", platform, "foo-bar"),
        Some("2.0.0".into())
    );
    assert_eq!(
        lock.get_pypi_package_version("older", platform, "foo-bar"),
        Some("3.0.0".into())
    );
    pixi.lock()
        .with_check(true)
        .with_pypi_constraint("foo-bar>=3; python_version < '3.12'")
        .await
        .unwrap();
    pixi.lock().with_check(true).await.unwrap();
    assert_eq!(pixi.manifest_contents().unwrap(), manifest_before);
}

#[tokio::test]
async fn platform_constraints_override_and_feature_constraints_intersect() {
    setup_tracing();
    let mut packages = MockRepoData::default();
    for platform in [Subdir::Linux64, Subdir::Win64] {
        packages.add_package(
            Package::build("python", "3.12.0")
                .with_subdir(platform)
                .finish(),
        );
    }
    let channel = packages.into_channel().await.unwrap();
    let index = PyPIDatabase::new()
        .with(PyPIPackage::new("consumer", "1.0.0").with_requires_dist(["foo>=1"]))
        .with(PyPIPackage::new("foo", "1.0.0"))
        .with(PyPIPackage::new("foo", "2.0.0"))
        .with(PyPIPackage::new("foo", "3.0.0"))
        .into_simple_index()
        .unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel}"]
        platforms = ["linux-64", "win-64"]
        conda-pypi-map = false
        [dependencies]
        python = "==3.12.0"
        [pypi-dependencies]
        consumer = "*"
        [pypi-options]
        index-url = "{index}"
        [pypi-constraints]
        foo = "==1"
        [target.linux-64.pypi-constraints]
        foo = ">=2"
        [feature.cap.pypi-constraints]
        foo = "<3"
        [environments]
        capped = ["cap"]
        [environments.inline.pypi-constraints]
        foo = "<3"
        "#,
        channel = channel.url(),
        index = index.index_url(),
    ))
    .unwrap();
    let lock = pixi.update_lock_file().await.unwrap();
    for (environment, linux_version) in [
        ("default", "3.0.0"),
        ("capped", "2.0.0"),
        ("inline", "2.0.0"),
    ] {
        assert_eq!(
            lock.get_pypi_package_version(environment, Subdir::Linux64, "foo"),
            Some(linux_version.into())
        );
        assert_eq!(
            lock.get_pypi_package_version(environment, Subdir::Win64, "foo"),
            Some("1.0.0".into())
        );
    }
    pixi.lock().with_check(true).await.unwrap();
}

#[tokio::test]
async fn constraint_only_feature_updates_the_entire_solve_group() {
    setup_tracing();
    let platform = Subdir::current().unwrap_or(Subdir::NoArch);
    let mut packages = MockRepoData::default();
    packages.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = packages.into_channel().await.unwrap();
    let index = PyPIDatabase::new()
        .with(PyPIPackage::new("consumer", "1.0.0").with_requires_dist(["foo>=1"]))
        .with(PyPIPackage::new("foo", "2.0.0"))
        .with(PyPIPackage::new("foo", "3.0.0"))
        .into_simple_index()
        .unwrap();
    let manifest = |bound: &str| {
        format!(
            r#"
            [workspace]
            channels = ["{channel}"]
            platforms = ["{platform}"]
            conda-pypi-map = false
            [dependencies]
            python = "==3.12.0"
            [pypi-dependencies]
            consumer = "*"
            [pypi-options]
            index-url = "{index}"
            [feature.guard.dependencies]
            python = "==3.12.0"
            [feature.guard.pypi-constraints]
            foo = "{bound}"
            absent-package = ">=999"
            [environments]
            shared = {{ solve-group = "shared" }}
            guarded = {{ features = ["guard"], solve-group = "shared", no-default-feature = true }}
            "#,
            channel = channel.url(),
            index = index.index_url(),
        )
    };
    let pixi = PixiControl::from_manifest(&manifest("*")).unwrap();
    let lock = pixi.update_lock_file().await.unwrap();
    for environment in ["default", "shared"] {
        assert_eq!(
            lock.get_pypi_package_version(environment, platform, "foo"),
            Some("3.0.0".into())
        );
    }
    assert!(!lock.contains_pypi_package("guarded", platform, "foo"));

    pixi.update_manifest(&manifest("<3")).unwrap();
    let lock_path = pixi.workspace_path().join("pixi.lock");
    let before = fs_err::read(&lock_path).unwrap();
    pixi.lock()
        .with_check(true)
        .await
        .expect_err("a constraint-only feature must invalidate its solve group");
    assert_eq!(fs_err::read(&lock_path).unwrap(), before);
    pixi.lock().await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    for (environment, version) in [("default", "3.0.0"), ("shared", "2.0.0")] {
        assert_eq!(
            lock.get_pypi_package_version(environment, platform, "foo"),
            Some(version.into())
        );
        assert!(!lock.contains_pypi_package(environment, platform, "absent-package"));
    }
    assert!(!lock.contains_pypi_package("guarded", platform, "foo"));
    pixi.lock().with_check(true).await.unwrap();
}

#[tokio::test]
async fn constraints_preserve_requirements_and_restrict_dependency_overrides() {
    setup_tracing();
    let platform = Subdir::current().unwrap_or(Subdir::NoArch);
    let mut packages = MockRepoData::default();
    packages.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = packages.into_channel().await.unwrap();
    let index = PyPIDatabase::new()
        .with(PyPIPackage::new("consumer", "1.0.0").with_requires_dist(["foo<2"]))
        .with(PyPIPackage::new("foo", "1.0.0"))
        .with(PyPIPackage::new("foo", "2.0.0"))
        .with(PyPIPackage::new("foo", "3.0.0"))
        .into_simple_index()
        .unwrap();
    let manifest = |constraint: &str, overrides: &str| {
        format!(
            r#"
            [workspace]
            channels = ["{channel}"]
            platforms = ["{platform}"]
            conda-pypi-map = false
            [dependencies]
            python = "==3.12.0"
            [pypi-dependencies]
            consumer = "*"
            [pypi-options]
            index-url = "{index}"
            [pypi-constraints]
            foo = "{constraint}"
            {overrides}
            "#,
            channel = channel.url(),
            index = index.index_url(),
        )
    };
    let pixi = PixiControl::from_manifest(&manifest("*", "")).unwrap();
    let lock = pixi.update_lock_file().await.unwrap();
    assert_eq!(
        lock.get_pypi_package_version("default", platform, "foo"),
        Some("1.0.0".into())
    );
    let lock_path = pixi.workspace_path().join("pixi.lock");
    let before = fs_err::read(&lock_path).unwrap();
    pixi.update_manifest(&manifest(">=2", "")).unwrap();
    pixi.lock()
        .await
        .expect_err("a constraint must not relax the consumer's foo<2 requirement");
    assert_eq!(fs_err::read(&lock_path).unwrap(), before);

    pixi.update_manifest(&manifest(
        ">=2,<3",
        "[pypi-options.dependency-overrides]\nfoo = \">=2\"",
    ))
    .unwrap();
    pixi.lock().await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert_eq!(
        lock.get_pypi_package_version("default", platform, "foo"),
        Some("2.0.0".into())
    );
    pixi.lock().with_check(true).await.unwrap();
    let before = fs_err::read(&lock_path).unwrap();
    pixi.lock()
        .with_pypi_constraint("foo<2")
        .await
        .expect_err("dependency overrides must not discard temporary constraints");
    assert_eq!(fs_err::read(&lock_path).unwrap(), before);
}

#[tokio::test]
async fn known_minimum_prerelease_version_satisfies_constraints() {
    setup_tracing();
    let platform = Subdir::current().unwrap_or(Subdir::NoArch);
    let mut packages = MockRepoData::default();
    packages.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(platform)
            .finish(),
    );
    let channel = packages.into_channel().await.unwrap();
    let index = PyPIDatabase::new()
        .with(PyPIPackage::new("foo", "0a0.dev0"))
        .into_simple_index()
        .unwrap();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
        [workspace]
        channels = ["{channel}"]
        platforms = ["{platform}"]
        conda-pypi-map = false
        [dependencies]
        python = "==3.12.0"
        [pypi-dependencies]
        foo = "==0a0.dev0"
        [pypi-options]
        index-url = "{index}"
        "#,
        channel = channel.url(),
        index = index.index_url(),
    ))
    .unwrap();
    pixi.lock().await.unwrap();
    let lock = pixi.lock_file().await.unwrap();
    assert_eq!(
        lock.get_pypi_package_version("default", platform, "foo"),
        Some("0a0.dev0".into())
    );
    let lock_path = pixi.workspace_path().join("pixi.lock");
    let before = fs_err::read(&lock_path).unwrap();
    pixi.lock()
        .with_check(true)
        .with_pypi_constraint("foo==0a0.dev0")
        .await
        .unwrap();
    pixi.lock()
        .with_pypi_constraint("foo==0a0.dev0")
        .await
        .unwrap();
    assert_eq!(fs_err::read(&lock_path).unwrap(), before);
}
