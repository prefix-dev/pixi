use std::{
    collections::{BTreeSet, HashMap},
    str::FromStr,
};

use pixi::pypi_mapping::{self};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_lock::DEFAULT_ENVIRONMENT_NAME;
use serial_test::serial;
use tempfile::TempDir;
use url::Url;

use crate::common::{
    builders::HasDependencyConfig,
    package_database::{Package, PackageDatabase},
    LockFileExt, PixiControl,
};

mod common;

#[tokio::test]
async fn conda_solve_group_functionality() {
    let mut package_database = PackageDatabase::default();

    // Add a package `foo` with 3 different versions
    package_database.add_package(Package::build("foo", "1").finish());
    package_database.add_package(Package::build("foo", "2").finish());
    package_database.add_package(Package::build("foo", "3").finish());

    // Add a package `bar` with 1 version that restricts `foo` to version 2 or
    // lower.
    package_database.add_package(
        Package::build("bar", "1")
            .with_dependency("foo <3")
            .finish(),
    );

    // Write the repodata to disk
    let channel_dir = TempDir::new().unwrap();
    package_database
        .write_repodata(channel_dir.path())
        .await
        .unwrap();

    let channel = Url::from_file_path(channel_dir.path()).unwrap();
    let platform = Platform::current();
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-solve-group"
    channels = ["{channel}"]
    platforms = ["{platform}"]

    [dependencies]
    foo = "*"

    [feature.test.dependencies]
    bar = "*"

    [environments]
    prod = {{ solve-group = "prod" }}
    test = {{ features=["test"], solve-group = "prod" }}
    "#
    ))
    .unwrap();

    // Get an up-to-date lockfile
    let lock_file = pixi.up_to_date_lock_file().await.unwrap();

    assert!(
        lock_file.contains_match_spec("default", platform, "foo ==3"),
        "default should have the highest version of foo"
    );
    assert!(
        !lock_file.contains_match_spec("default", platform, "bar"),
        "default should not contain bar"
    );

    assert!(
        lock_file.contains_match_spec("prod", platform, "foo ==2"),
        "prod should have foo==2 because it shares the solve group with test"
    );
    assert!(
        !lock_file.contains_match_spec("prod", platform, "bar"),
        "prod should not contain bar"
    );

    assert!(
        lock_file.contains_match_spec("test", platform, "foo ==2"),
        "test should have foo==2 because bar depends on foo <3"
    );
    assert!(
        lock_file.contains_match_spec("test", platform, "bar"),
        "test should contain bar"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[serial]
async fn test_purl_are_added_for_pypi() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("boltons").with_install(true).await.unwrap();

    let lock_file = pixi.up_to_date_lock_file().await.unwrap();

    // Check if boltons has a purl
    lock_file
        .default_environment()
        .unwrap()
        .packages(Platform::current())
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().package_record().name
                == PackageName::from_str("boltons").unwrap()
            {
                assert!(dep.as_conda().unwrap().package_record().purls.is_none());
            }
        });

    // Add boltons from pypi
    pixi.add("boltons")
        .with_install(true)
        .set_type(pixi::DependencyType::PypiDependency)
        .await
        .unwrap();

    let lock_file = pixi.up_to_date_lock_file().await.unwrap();

    // Check if boltons has a purl
    lock_file
        .default_environment()
        .unwrap()
        .packages(Platform::current())
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().package_record().name
                == PackageName::from_str("boltons").unwrap()
            {
                assert!(!dep
                    .as_conda()
                    .unwrap()
                    .package_record()
                    .purls
                    .as_ref()
                    .unwrap()
                    .is_empty());
            }
        });

    // Check if boltons exists only as conda dependency
    assert!(lock_file.contains_match_spec(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "boltons"
    ));
    assert!(!lock_file.contains_pypi_package(
        DEFAULT_ENVIRONMENT_NAME,
        Platform::current(),
        "boltons"
    ));
}

#[tokio::test]
async fn test_purl_are_generated_using_custom_mapping() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.project().unwrap();
    let client = project.authenticated_client();
    let foo_bar_package = Package::build("foo-bar-car", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "foo-bar-car".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: "dummy-channel".to_owned(),
    };

    let packages = vec![repo_data_record.clone()];

    let conda_mapping =
        pypi_mapping::prefix_pypi_name_mapping::conda_pypi_name_mapping(client, &packages, None)
            .await
            .unwrap();
    // We are using custom mapping
    let compressed_mapping =
        HashMap::from([("foo-bar-car".to_owned(), Some("my-test-name".to_owned()))]);

    pypi_mapping::prefix_pypi_name_mapping::amend_pypi_purls_for_record(
        &mut repo_data_record,
        &conda_mapping,
        &compressed_mapping,
    )
    .unwrap();

    let first_purl = repo_data_record
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // We verify that `my-test-name` is used for `foo-bar-car` package
    assert_eq!(first_purl.name(), "my-test-name")
}

#[tokio::test]
async fn test_compressed_mapping_catch_not_pandoc_not_a_python_package() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.project().unwrap();
    let client = project.authenticated_client();
    let foo_bar_package = Package::build("pandoc", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pandoc".to_owned(),
        url: Url::parse("https://haskell.org/pandoc/").unwrap(),
        channel: "conda-forge".to_owned(),
    };

    let packages = vec![repo_data_record.clone()];

    let conda_mapping =
        pypi_mapping::prefix_pypi_name_mapping::conda_pypi_name_mapping(client, &packages, None)
            .await
            .unwrap();

    let compressed_mapping =
        pypi_mapping::prefix_pypi_name_mapping::conda_pypi_name_compressed_mapping(client)
            .await
            .unwrap();

    pypi_mapping::prefix_pypi_name_mapping::amend_pypi_purls_for_record(
        &mut repo_data_record,
        &conda_mapping,
        &compressed_mapping,
    )
    .unwrap();

    // pandoc is not a python package
    // so purls for it should be empty
    assert!(repo_data_record.package_record.purls.unwrap().is_empty())
}

#[tokio::test]
async fn test_dont_record_not_present_package_as_purl() {
    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.project().unwrap();
    let client = project.authenticated_client();
    // We use one package that is present in our mapping: `boltons`
    // and another one that is missing from conda and our mapping:
    // `pixi-something-new-for-test` because `pixi-something-new-for-test` is
    // from conda-forge channel we will anyway record a purl for it
    // by assumption that it's a pypi package
    let foo_bar_package = Package::build("pixi-something-new-for-test", "2").finish();
    let boltons_package = Package::build("boltons", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new-for-test".to_owned(),
        url: Url::parse("https://pypi.org/simple/something-new/").unwrap(),
        channel: "https://conda.anaconda.org/conda-forge/osx-arm64/brotli-python-1.1.0-py311ha891d26_1.conda".to_owned(),
    };

    let mut boltons_repo_data_record = RepoDataRecord {
        package_record: boltons_package.package_record,
        file_name: "boltons".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: "https://conda.anaconda.org/conda-forge/".to_owned(),
    };

    let packages = vec![repo_data_record.clone(), boltons_repo_data_record.clone()];

    let conda_mapping =
        pypi_mapping::prefix_pypi_name_mapping::conda_pypi_name_mapping(client, &packages, None)
            .await
            .unwrap();

    let compressed_mapping =
        pypi_mapping::prefix_pypi_name_mapping::conda_pypi_name_compressed_mapping(client)
            .await
            .unwrap();

    pypi_mapping::prefix_pypi_name_mapping::amend_pypi_purls_for_record(
        &mut repo_data_record,
        &conda_mapping,
        &compressed_mapping,
    )
    .unwrap();

    pypi_mapping::prefix_pypi_name_mapping::amend_pypi_purls_for_record(
        &mut boltons_repo_data_record,
        &conda_mapping,
        &compressed_mapping,
    )
    .unwrap();

    let first_purl = repo_data_record
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // we verify that even if this name is not present in our mapping
    // we anyway record a purl because we make an assumption
    // that it's a pypi package
    assert_eq!(first_purl.name(), "pixi-something-new-for-test");

    let boltons_purl = boltons_repo_data_record
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // for boltons we have a mapping record
    // so we test that we also record source=conda-forge-mapping qualifier
    assert_eq!(
        boltons_purl.qualifiers().get("source").unwrap(),
        "conda-forge-mapping"
    );
}

#[tokio::test]
async fn test_we_record_not_present_package_as_purl_for_custom_mapping() {
    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-channel-change"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { 'conda-forge' = "tests/mapping_files/compressed_mapping.json" }
    "#,
    )
    .unwrap();

    let project = pixi.project().unwrap();

    let client = project.authenticated_client();

    // We use one package that is present in our mapping: `boltons`
    // and another one that is missing from conda and our mapping:
    // `pixi-something-new-for-test` because `pixi-something-new-for-test` is
    // from conda-forge channel we will anyway record a purl for it
    // by assumption that it's a pypi package
    // also we are using some custom mapping
    // so we will test for other purl qualifier comparing to
    // `test_dont_record_not_present_package_as_purl` test
    let foo_bar_package = Package::build("pixi-something-new", "2").finish();
    let boltons_package = Package::build("boltons", "2").finish();

    let repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new".to_owned(),
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: "https://conda.anaconda.org/conda-forge/".to_owned(),
    };

    let boltons_repo_data_record = RepoDataRecord {
        package_record: boltons_package.package_record,
        file_name: "boltons".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: "https://conda.anaconda.org/conda-forge/".to_owned(),
    };

    let mut packages = vec![repo_data_record, boltons_repo_data_record];

    let mapping_map = project.pypi_name_mapping_source().custom().unwrap();

    pypi_mapping::custom_pypi_mapping::amend_pypi_purls(client, &mapping_map, &mut packages, None)
        .await
        .unwrap();

    let boltons_package = packages.pop().unwrap();

    let boltons_first_purl = boltons_package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // for boltons we have a mapping record
    // so we test that we also record source=project-defined-mapping qualifier
    assert_eq!(boltons_first_purl.name(), "boltons");
    assert_eq!(
        boltons_first_purl.qualifiers().get("source").unwrap(),
        "project-defined-mapping"
    );

    let package = packages.pop().unwrap();

    let first_purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // we verify that even if this name is not present in our mapping
    // we anyway record a purl because we make an assumption
    // that it's a pypi package
    assert_eq!(first_purl.name(), "pixi-something-new");
    assert!(first_purl.qualifiers().is_empty());
}

#[tokio::test]
async fn test_custom_mapping_channel_with_suffix() {
    let pixi = PixiControl::from_manifest(r#"
     [project]
     name = "test-channel-change"
     channels = ["conda-forge"]
     platforms = ["linux-64"]
     conda-pypi-map = { "https://conda.anaconda.org/conda-forge/" = "tests/mapping_files/custom_mapping.json" }
     "#,
    )
    .unwrap();

    let project = pixi.project().unwrap();

    let client = project.authenticated_client();

    let foo_bar_package = Package::build("pixi-something-new", "2").finish();

    let repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new".to_owned(),
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: "https://conda.anaconda.org/conda-forge".to_owned(),
    };

    let mut packages = vec![repo_data_record];

    let mapping_source = project.pypi_name_mapping_source();

    let mapping_map = mapping_source.custom().unwrap();

    pypi_mapping::custom_pypi_mapping::amend_pypi_purls(client, &mapping_map, &mut packages, None)
        .await
        .unwrap();

    let package = packages.pop().unwrap();

    assert_eq!(
        package
            .package_record
            .purls
            .as_ref()
            .and_then(BTreeSet::first)
            .unwrap()
            .qualifiers()
            .get("source")
            .unwrap(),
        "project-defined-mapping"
    );
}

#[tokio::test]
async fn test_repo_data_record_channel_with_suffix() {
    let pixi = PixiControl::from_manifest(r#"
     [project]
     name = "test-channel-change"
     channels = ["conda-forge"]
     platforms = ["linux-64"]
     conda-pypi-map = { "https://conda.anaconda.org/conda-forge" = "tests/mapping_files/custom_mapping.json" }
     "#,
    )
    .unwrap();

    let project = pixi.project().unwrap();

    let client = project.authenticated_client();

    let foo_bar_package = Package::build("pixi-something-new", "2").finish();

    let repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new".to_owned(),
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: "https://conda.anaconda.org/conda-forge/".to_owned(),
    };

    let mut packages = vec![repo_data_record];

    let mapping_source = project.pypi_name_mapping_source();

    let mapping_map = mapping_source.custom().unwrap();

    pypi_mapping::custom_pypi_mapping::amend_pypi_purls(client, &mapping_map, &mut packages, None)
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    assert_eq!(
        package
            .package_record
            .purls
            .as_ref()
            .and_then(BTreeSet::first)
            .unwrap()
            .qualifiers()
            .get("source")
            .unwrap(),
        "project-defined-mapping"
    );
}
