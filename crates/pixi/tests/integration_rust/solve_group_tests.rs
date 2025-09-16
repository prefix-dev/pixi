use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    str::FromStr,
    sync::Arc,
};

use pypi_mapping::{self, CustomMapping, MappingLocation, MappingSource, PurlSource};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_lock::DEFAULT_ENVIRONMENT_NAME;
use reqwest_middleware::ClientBuilder;
use tempfile::TempDir;
use url::Url;

use crate::common::{
    LockFileExt, PixiControl,
    builders::{HasDependencyConfig, HasNoInstallConfig},
    client::OfflineMiddleware,
    package_database::{Package, PackageDatabase},
};
use crate::setup_tracing;

#[tokio::test]
async fn conda_solve_group_functionality() {
    setup_tracing();

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
    let lock_file = pixi.update_lock_file().await.unwrap();

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
#[cfg_attr(not(feature = "slow_integration_tests"), ignore)]
async fn test_purl_are_added_for_pypi() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lockfile with this version of python
    pixi.add("boltons").await.unwrap();
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Check if boltons has a purl
    lock_file
        .default_environment()
        .unwrap()
        .packages(Platform::current())
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().record().name == PackageName::from_str("boltons").unwrap() {
                assert!(dep.as_conda().unwrap().record().purls.is_none());
            }
        });

    // Add boltons from pypi
    pixi.add("boltons")
        .with_install(true)
        .set_type(pixi_core::DependencyType::PypiDependency)
        .await
        .unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();

    // Check if boltons has a purl
    lock_file
        .default_environment()
        .unwrap()
        .packages(Platform::current())
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().record().name == PackageName::from_str("boltons").unwrap() {
                assert_eq!(
                    dep.as_conda()
                        .unwrap()
                        .record()
                        .purls
                        .as_ref()
                        .unwrap()
                        .first()
                        .unwrap()
                        .qualifiers()
                        .get("source")
                        .unwrap(),
                    PurlSource::HashMapping.as_str()
                );
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
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_purl_are_missing_for_non_conda_forge() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let foo_bar_package = Package::build("foo-bar-car", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "foo-bar-car".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("dummy-channel".to_owned()),
    };

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(&MappingSource::Prefix, vec![&mut repo_data_record], None)
        .await
        .unwrap();

    // Because foo-bar-car is not from conda-forge channel
    // We verify that purls are missing for non-conda-forge packages
    assert!(
        repo_data_record
            .package_record
            .purls
            .as_ref()
            .and_then(BTreeSet::first)
            .is_none()
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_purl_are_generated_using_custom_mapping() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let foo_bar_package = Package::build("foo-bar-car", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "foo-bar-car".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    // We are using custom mapping
    let compressed_mapping =
        HashMap::from([("foo-bar-car".to_owned(), Some("my-test-name".to_owned()))]);
    let source = HashMap::from([(
        "https://conda.anaconda.org/conda-forge".to_owned(),
        MappingLocation::Memory(compressed_mapping),
    )]);

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(
            &MappingSource::Custom(Arc::new(CustomMapping::new(source))),
            vec![&mut repo_data_record],
            None,
        )
        .await
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
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_compressed_mapping_catch_not_pandoc_not_a_python_package() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let foo_bar_package = Package::build("pandoc", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pandoc".to_owned(),
        url: Url::parse("https://haskell.org/pandoc/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let packages = vec![&mut repo_data_record];

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(&MappingSource::Prefix, packages, None)
        .await
        .unwrap();

    // pandoc is not a python package
    // so purls for it should be empty
    assert!(repo_data_record.package_record.purls.unwrap().is_empty())
}

#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_dont_record_not_present_package_as_purl() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    // We use one package that is present in our mapping: `boltons`
    // and another one that is missing from conda and our mapping:
    // `pixi-something-new-for-test` because `pixi-something-new-for-test` is
    // from conda-forge channel we will anyway record a purl for it
    // by assumption that it's a pypi package
    let foo_bar_package = Package::build("pixi-something-new-for-test", "2").finish();
    // We use one package that is not present by hash
    // but `boltons` name is still present in compressed mapping
    // so we will record a purl for it
    let boltons_package = Package::build("boltons", "99999").finish();

    let mut repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new-for-test".to_owned(),
        url: Url::parse("https://pypi.org/simple/something-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/osx-arm64/brotli-python-1.1.0-py311ha891d26_1.conda".to_owned()),
    };

    let mut boltons_repo_data_record = RepoDataRecord {
        package_record: boltons_package.package_record,
        file_name: "boltons".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            vec![&mut repo_data_record, &mut boltons_repo_data_record],
            None,
        )
        .await
        .unwrap();

    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            vec![&mut repo_data_record, &mut boltons_repo_data_record],
            None,
        )
        .await
        .unwrap();

    let first_purl = repo_data_record
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // we verify that even if this name is not present in our mapping
    // we record a purl anyways. Because we make the assumption
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
        PurlSource::CompressedMapping.as_str()
    );
}

fn absolute_custom_mapping_path() -> String {
    dunce::simplified(
        &Path::new(env!("CARGO_WORKSPACE_DIR"))
            .join("tests/data/mapping_files/custom_mapping.json"),
    )
    .display()
    .to_string()
    .replace("\\", "/")
}

fn absolute_compressed_mapping_path() -> String {
    dunce::simplified(
        &Path::new(env!("CARGO_WORKSPACE_DIR"))
            .join("tests/data/mapping_files/compressed_mapping.json"),
    )
    .display()
    .to_string()
    .replace("\\", "/")
}

#[tokio::test]
async fn test_we_record_not_present_package_as_purl_for_custom_mapping() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-channel-change"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ 'conda-forge' = "{}" }}
    "#,
        absolute_compressed_mapping_path()
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

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
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let boltons_repo_data_record = RepoDataRecord {
        package_record: boltons_package.package_record,
        file_name: "boltons".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![repo_data_record, boltons_repo_data_record];

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let boltons_package = packages.pop().unwrap();

    let boltons_first_purl = boltons_package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    println!("{boltons_first_purl}");

    // for boltons we have a mapping record
    // so we test that we also record source=project-defined-mapping qualifier
    assert_eq!(boltons_first_purl.name(), "boltons");
    assert_eq!(
        boltons_first_purl.qualifiers().get("source").unwrap(),
        PurlSource::ProjectDefinedMapping.as_str()
    );

    let package = packages.pop().unwrap();

    let first_purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // we verify that even if this name is not present in our mapping
    // we record a purl anyways. Because we make the assumption
    // that it's a pypi package
    assert_eq!(first_purl.name(), "pixi-something-new");
    assert!(first_purl.qualifiers().is_empty());
}

#[tokio::test]
async fn test_custom_mapping_channel_with_suffix() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
     [project]
     name = "test-channel-change"
     channels = ["conda-forge"]
     platforms = ["linux-64"]
     conda-pypi-map = {{ "https://conda.anaconda.org/conda-forge/" = "{}" }}
     "#,
        absolute_custom_mapping_path()
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

    let foo_bar_package = Package::build("pixi-something-new", "2").finish();

    let repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new".to_owned(),
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            &mut packages,
            None,
        )
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
        PurlSource::ProjectDefinedMapping.as_str()
    );
}

#[tokio::test]
async fn test_repo_data_record_channel_with_suffix() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
     [project]
     name = "test-channel-change"
     channels = ["conda-forge"]
     platforms = ["linux-64"]
     conda-pypi-map = {{ "https://conda.anaconda.org/conda-forge" = "{}" }}
     "#,
        absolute_custom_mapping_path(),
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

    let foo_bar_package = Package::build("pixi-something-new", "2").finish();

    let repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new".to_owned(),
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            &mut packages,
            None,
        )
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
        PurlSource::ProjectDefinedMapping.as_str()
    );
}

#[tokio::test]
async fn test_path_channel() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
     [project]
     name = "test-channel-change"
     channels = ["file:///home/user/staged-recipes/build_artifacts"]
     platforms = ["linux-64"]
     conda-pypi-map = {{"file:///home/user/staged-recipes/build_artifacts" = "{}" }}
     "#,
        absolute_custom_mapping_path()
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

    let foo_bar_package = Package::build("pixi-something-new", "2").finish();

    let repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new".to_owned(),
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("file:///home/user/staged-recipes/build_artifacts".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            &mut packages,
            None,
        )
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
        PurlSource::ProjectDefinedMapping.as_str()
    );
}

#[tokio::test]
async fn test_file_url_as_mapping_location() {
    setup_tracing();

    let tmp_dir = tempfile::tempdir().unwrap();
    let mapping_file = tmp_dir.path().join("custom_mapping.json");

    let _ = fs_err::write(
        &mapping_file,
        r#"
    {
        "pixi-something-new": "pixi-something-old"
    }
    "#,
    );

    let mapping_file_path_as_url = Url::from_file_path(
        mapping_file, /* .canonicalize()
                       * .expect("should be canonicalized"), */
    )
    .unwrap();

    let pixi = PixiControl::from_manifest(
        format!(
            r#"
        [project]
        name = "test-channel-change"
        channels = ["conda-forge"]
        platforms = ["linux-64"]
        conda-pypi-map = {{"conda-forge" = "{}"}}
        "#,
            mapping_file_path_as_url.as_str()
        )
        .as_str(),
    )
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

    let foo_bar_package = Package::build("pixi-something-new", "2").finish();

    let repo_data_record = RepoDataRecord {
        package_record: foo_bar_package.package_record,
        file_name: "pixi-something-new".to_owned(),
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::MappingClient::builder(client.clone()).finish();
    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            &mut packages,
            None,
        )
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
        PurlSource::ProjectDefinedMapping.as_str()
    );
}

#[tokio::test]
async fn test_disabled_mapping() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-channel-change"
    channels = ["https://prefix.dev/conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

    let blocking_middleware = OfflineMiddleware;

    let blocked_client = ClientBuilder::from_client(client.clone())
        .with(blocking_middleware)
        .build();

    let boltons_package = Package::build("boltons", "2").finish();

    let boltons_repo_data_record = RepoDataRecord {
        package_record: boltons_package.package_record,
        file_name: "boltons".to_owned(),
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![boltons_repo_data_record];

    let mapping_client = pypi_mapping::MappingClient::builder(blocked_client).finish();
    mapping_client
        .amend_purls(
            project.pypi_name_mapping_source().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let boltons_package = packages.pop().unwrap();

    let boltons_first_purl = boltons_package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();

    // we verify that even if this name is not present in our mapping
    // we record a purl anyways. Because we make the assumption
    // that it's a pypi package
    assert_eq!(boltons_first_purl.name(), "boltons");
    assert!(boltons_first_purl.qualifiers().is_empty());
}
