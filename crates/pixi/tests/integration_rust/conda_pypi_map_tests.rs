//! Tests for the conda↔PyPI name mapping: purl derivation through the
//! prefix.dev chain, project-defined `conda-pypi-map` overrides in their
//! extend/replace/disabled modes, and the `cache-ttl` mapping cache.

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    str::FromStr,
    sync::Arc,
};

use pypi_mapping::{
    self, ProjectDefinedChannelMapping, ProjectDefinedMapping, ProjectDefinedMappingLocation,
    PurlDerivationMode, PurlDerivationSource, PypiNames,
};
use rattler_conda_types::{PackageName, Platform, RepoDataRecord};
use rattler_lock::DEFAULT_ENVIRONMENT_NAME;
use reqwest_middleware::ClientBuilder;
use tempfile::TempDir;
use url::Url;

use crate::common::{
    LockFileExt, PixiControl,
    builders::HasDependencyConfig,
    client::OfflineMiddleware,
    pypi_index::{Database as PyPIDatabase, PyPIPackage},
};
use crate::setup_tracing;
use pixi_test_utils::{MockRepoData, Package};

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
#[cfg_attr(
    any(not(feature = "online_tests"), not(feature = "slow_integration_tests")),
    ignore
)]
async fn test_purl_are_added_for_pypi() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    // Add and update lock file with this version of python
    pixi.add("boltons").await.unwrap();
    let lock_file = pixi.update_lock_file().await.unwrap();

    // Check if boltons has a purl
    let p = lock_file
        .platform(&Platform::current().to_string())
        .unwrap();
    lock_file
        .default_environment()
        .unwrap()
        .packages(p)
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().name() == &PackageName::from_str("boltons").unwrap() {
                assert!(dep.as_conda().unwrap().record().unwrap().purls.is_none());
            }
        });

    // Add boltons from pypi
    pixi.add("boltons")
        .set_type(pixi_core::DependencyType::PypiDependency)
        .await
        .unwrap();

    let lock_file = pixi.update_lock_file().await.unwrap();

    // Check if boltons has a purl
    let p = lock_file
        .platform(&Platform::current().to_string())
        .unwrap();
    lock_file
        .default_environment()
        .unwrap()
        .packages(p)
        .unwrap()
        .for_each(|dep| {
            if dep.as_conda().unwrap().name() == &PackageName::from_str("boltons").unwrap() {
                assert_eq!(
                    dep.as_conda()
                        .and_then(|c| c.as_binary())
                        .and_then(|c| c.package_record.purls.as_ref())
                        .unwrap()
                        .first()
                        .unwrap()
                        .qualifiers()
                        .get("source")
                        .unwrap(),
                    PurlDerivationSource::PrefixHashMapping.as_str()
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
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("dummy-channel".to_owned()),
    };

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            &PurlDerivationMode::Prefix,
            vec![&mut repo_data_record],
            None,
        )
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
async fn test_purl_are_generated_using_custom_mapping() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let foo_bar_package = Package::build("foo-bar-car", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    // We are using project-defined mapping
    let compressed_mapping = HashMap::from([(
        "foo-bar-car".to_owned(),
        PypiNames(vec!["my-test-name".to_owned()]),
    )]);
    let source = HashMap::from([(
        "https://conda.anaconda.org/conda-forge".to_owned(),
        ProjectDefinedChannelMapping::replace(ProjectDefinedMappingLocation::InMemory(
            compressed_mapping,
        )),
    )]);

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            &PurlDerivationMode::ProjectDefined(Arc::new(ProjectDefinedMapping::new(source))),
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
async fn test_multiple_pypi_names_generate_multiple_purls() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let package = Package::build("ambertools", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        identifier: package.identifier(),
        package_record: package.package_record,
        url: Url::parse("https://conda.anaconda.org/conda-forge/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    // One conda package providing several PyPI distributions, the
    // parselmouth `files/v0` list format.
    let compressed_mapping = HashMap::from([(
        "ambertools".to_owned(),
        PypiNames(vec!["parmed".to_owned(), "pytraj".to_owned()]),
    )]);
    let source = HashMap::from([(
        "https://conda.anaconda.org/conda-forge".to_owned(),
        ProjectDefinedChannelMapping::replace(ProjectDefinedMappingLocation::InMemory(
            compressed_mapping,
        )),
    )]);

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            &PurlDerivationMode::ProjectDefined(Arc::new(ProjectDefinedMapping::new(source))),
            vec![&mut repo_data_record],
            None,
        )
        .await
        .unwrap();

    let purls = repo_data_record.package_record.purls.as_ref().unwrap();
    let purl_names: Vec<&str> = purls.iter().map(|purl| purl.name()).collect();
    assert_eq!(purl_names, ["parmed", "pytraj"]);
    assert!(
        purls
            .iter()
            .all(|purl| purl.to_string().contains("source=project-defined-mapping"))
    );
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
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://haskell.org/pandoc/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let packages = vec![&mut repo_data_record];

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(&PurlDerivationMode::Prefix, packages, None)
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
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/something-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/osx-arm64/brotli-python-1.1.0-py311ha891d26_1.conda".to_owned()),
    };

    let mut boltons_repo_data_record = RepoDataRecord {
        identifier: boltons_package.identifier(),
        package_record: boltons_package.package_record,
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            vec![&mut repo_data_record, &mut boltons_repo_data_record],
            None,
        )
        .await
        .unwrap();

    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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
        PurlDerivationSource::PrefixCompressedMapping.as_str()
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
    conda-pypi-map = {{ 'conda-forge' = {{ location = "{}", mode = "replace" }} }}
    "#,
        absolute_compressed_mapping_path()
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

    // We use one package that is present in our mapping: `boltons`
    // and another one that is missing from conda and our mapping:
    // `pixi-something-new-for-test`. Because the mapping uses
    // `mode = "replace"` the mapping is exclusive: packages that are not in
    // it must not get a purl, not even the conda-forge verbatim fallback.
    let foo_bar_package = Package::build("pixi-something-new", "2").finish();
    let boltons_package = Package::build("boltons", "2").finish();

    let repo_data_record = RepoDataRecord {
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let boltons_repo_data_record = RepoDataRecord {
        identifier: boltons_package.identifier(),
        package_record: boltons_package.package_record,
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![repo_data_record, boltons_repo_data_record];

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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
        PurlDerivationSource::ProjectDefinedMapping.as_str()
    );

    let package = packages.pop().unwrap();

    // With a replace-mode project-defined mapping, packages not in the mapping
    // should NOT get purls. This verifies that replace mode is exclusive - only
    // packages explicitly mapped should be considered as pypi packages.
    assert!(
        package.package_record.purls.is_none()
            || package.package_record.purls.as_ref().unwrap().is_empty(),
        "pixi-something-new should not have purls when not in a replace-mode mapping"
    );
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
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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
        PurlDerivationSource::ProjectDefinedMapping.as_str()
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
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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
        PurlDerivationSource::ProjectDefinedMapping.as_str()
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
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("file:///home/user/staged-recipes/build_artifacts".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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
        PurlDerivationSource::ProjectDefinedMapping.as_str()
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
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/pixi-something-new-new/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![repo_data_record];

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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
        PurlDerivationSource::ProjectDefinedMapping.as_str()
    );
}

/// Build a `PurlDerivationClient` whose http client refuses any network
/// request, backed by the given cache directory.
fn offline_mapping_client(
    project: &pixi_core::Workspace,
    cache_dir: std::path::PathBuf,
) -> pypi_mapping::PurlDerivationClient {
    let client = project.authenticated_client().unwrap();
    let blocked_client = ClientBuilder::from_client(client.client().clone())
        .with(OfflineMiddleware)
        .build();
    pypi_mapping::PurlDerivationClient::builder(blocked_client.into(), cache_dir).finish()
}

fn conda_forge_record(name: &str) -> RepoDataRecord {
    let package = Package::build(name, "2").finish();
    RepoDataRecord {
        identifier: package.identifier(),
        package_record: package.package_record,
        url: Url::parse(&format!("https://pypi.org/simple/{name}/")).unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    }
}

/// An inline mapping hit in the default (extend) mode is final and requires
/// no network access.
#[tokio::test]
async fn test_extend_mapping_inline_hit_without_network() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-extend-inline"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { conda-forge = { mapping = { pixi-something-new = "my-inline-name" } } }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();
    assert_eq!(purl.name(), "my-inline-name");
    assert_eq!(
        purl.qualifiers().get("source").unwrap(),
        PurlDerivationSource::ProjectDefinedMapping.as_str()
    );
}

/// An explicit `false` inline entry means "not a PyPI package": no purl is
/// derived and the conda-forge verbatim fallback does not kick in either.
#[tokio::test]
async fn test_extend_mapping_explicit_false_yields_no_purl() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-extend-false"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { conda-forge = { mapping = { pixi-something-new = false } } }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    assert!(
        package
            .package_record
            .purls
            .as_ref()
            .is_none_or(|purls| purls.is_empty()),
        "a package explicitly mapped to `false` must not get a purl"
    );
}

/// `<channel> = false` disables lookups for that channel; the offline
/// conda-forge verbatim fallback still applies.
#[tokio::test]
async fn test_channel_disabled_keeps_verbatim_fallback() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-channel-disabled"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { conda-forge = false }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("boltons")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();
    // The verbatim fallback assumes the conda name is the pypi name and adds
    // no source qualifier.
    assert_eq!(purl.name(), "boltons");
    assert!(purl.qualifiers().is_empty());
}

/// Inline mapping keys are matched case-insensitively against the normalized
/// (lowercase) conda package names.
#[tokio::test]
async fn test_inline_mapping_keys_are_case_insensitive() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-inline-case"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { conda-forge = { mapping = { Pixi-Something-New = "mixed-case-win" } } }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .expect("a mixed-case inline key should match the lowercase record name");
    assert_eq!(purl.name(), "mixed-case-win");
}

/// The same channel spelled in two forms (by name and by URL) must be
/// rejected: the forms collapse to one channel after resolution and keeping
/// a nondeterministic winner would silently pick one of the two entries.
#[tokio::test]
async fn test_duplicate_channel_forms_are_rejected() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-duplicate-channel"
    channels = ["conda-forge"]
    platforms = ["linux-64"]

    [project.conda-pypi-map]
    conda-forge = false
    "https://conda.anaconda.org/conda-forge" = { mapping = { a = "b" } }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let err = project
        .pypi_name_derivation_mode()
        .expect_err("duplicate channel forms should be rejected");
    assert!(
        err.to_string().contains("more than once"),
        "error should mention the duplicate, got: {err}"
    );
}

/// `cache-ttl` combined with a local path location is rejected when the
/// derivation mode is built.
#[tokio::test]
async fn test_cache_ttl_on_local_path_is_rejected() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-ttl-local-path"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { conda-forge = { location = "mapping.json", cache-ttl = "24h" } }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let err = project
        .pypi_name_derivation_mode()
        .expect_err("cache-ttl on a local path should be rejected");
    assert!(
        err.to_string().contains("cache-ttl"),
        "error should mention cache-ttl, got: {err}"
    );
}

/// A `file://` url in the table-form `location` is normalized to a local
/// path and works like one.
#[tokio::test]
async fn test_file_url_in_table_location() {
    setup_tracing();

    let tmp_dir = tempfile::tempdir().unwrap();
    let mapping_file = tmp_dir.path().join("mapping.json");
    fs_err::write(
        &mapping_file,
        r#"{ "pixi-something-new": "from-file-url" }"#,
    )
    .unwrap();
    let mapping_url = Url::from_file_path(&mapping_file).unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-file-url-table"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ conda-forge = {{ location = "{mapping_url}", mode = "extend" }} }}
    "#,
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .expect("a file:// location should resolve like a local path");
    assert_eq!(purl.name(), "from-file-url");
}

/// When an entry has both a `location` and inline `mapping` entries, the
/// inline entries override the ones from the location.
#[tokio::test]
async fn test_inline_mapping_overrides_location() {
    setup_tracing();

    // The custom mapping file maps `pixi-something-new` to itself; the inline
    // entry must win.
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-inline-overrides"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ conda-forge = {{ location = "{}", mapping = {{ pixi-something-new = "inline-wins" }} }} }}
    "#,
        absolute_custom_mapping_path()
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();
    assert_eq!(purl.name(), "inline-wins");
}

/// In extend mode a miss in the project-defined mapping falls through to the
/// prefix.dev chain.
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_extend_mapping_miss_falls_through_to_prefix() {
    setup_tracing();

    // The mapping contains an unrelated package, so `boltons` is a miss and
    // must be resolved through the prefix.dev chain (the mock-built record's
    // hash is unknown there, so the compressed name mapping answers).
    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-extend-miss"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = { conda-forge = { mapping = { some-other-package = "other" } } }
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();

    let mut packages = vec![conda_forge_record("boltons")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();
    assert_eq!(purl.name(), "boltons");
    assert_eq!(
        purl.qualifiers().get("source").unwrap(),
        PurlDerivationSource::PrefixCompressedMapping.as_str()
    );
}

/// A mapping for one channel must not affect records from other channels:
/// they go through the full default chain, including the conda-forge verbatim
/// fallback.
///
/// This pins the per-record fallback behavior: previously, configuring any
/// `conda-pypi-map` suppressed the verbatim fallback globally, degrading purl
/// coverage even for channels that were not in the map.
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_mapping_for_other_channel_keeps_verbatim_fallback() {
    setup_tracing();

    // A replace-mode mapping for robostack only; the record below comes from
    // conda-forge and its name is unknown to both the prefix.dev hash and
    // compressed mappings, so only the verbatim fallback can answer.
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-unmapped-channel-verbatim"
    channels = ["conda-forge", "robostack"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ robostack = {{ location = "{}", mode = "replace" }} }}
    "#,
        absolute_custom_mapping_path()
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();
    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .expect("a record from an unmapped channel should get the verbatim fallback purl");
    // The verbatim fallback assumes the conda name is the pypi name and adds
    // no source qualifier.
    assert_eq!(purl.name(), "pixi-something-new");
    assert!(purl.qualifiers().is_empty());
}

/// The on-disk path of the TTL cache for a mapping url, mirroring the layout
/// used by the project-defined mapping resolver.
fn ttl_cache_path_for(cache_dir: &Path, url: &str) -> std::path::PathBuf {
    let hash = rattler_digest::compute_bytes_digest::<rattler_digest::Sha256>(url.as_bytes());
    cache_dir
        .join("project-defined")
        .join(format!("{hash:x}.json"))
}

fn manifest_with_ttl_mapping(url: &str, ttl: &str) -> String {
    format!(
        r#"
    [project]
    name = "test-cache-ttl"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ conda-forge = {{ location = "{url}", cache-ttl = "{ttl}" }} }}
    "#
    )
}

/// A cached mapping younger than `cache-ttl` is used without any network
/// access.
///
/// Note: with an offline client this test cannot distinguish the fresh-cache
/// path from the stale-fallback path (both serve the cached file). What it
/// pins is the on-disk cache layout (`project-defined/<sha256(url)>.json`)
/// and that a cached mapping is served without touching the network at all.
/// The fresh/expired age boundary itself is unit-tested in the
/// `pypi_mapping` crate (`read_ttl_cache`).
#[tokio::test]
async fn test_cache_ttl_fresh_cache_skips_network() {
    setup_tracing();

    let mapping_url = "https://example.invalid/mapping.json";
    let pixi = PixiControl::from_manifest(&manifest_with_ttl_mapping(mapping_url, "1h")).unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();

    // Pre-populate the TTL cache; the url itself is unreachable and the
    // client is offline, so a cache miss would fail the test.
    let cache_file = ttl_cache_path_for(cache_dir.path(), mapping_url);
    fs_err::create_dir_all(cache_file.parent().unwrap()).unwrap();
    fs_err::write(&cache_file, r#"{ "pixi-something-new": "from-the-cache" }"#).unwrap();

    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();
    assert_eq!(purl.name(), "from-the-cache");
}

/// When the cached mapping is expired and the refetch fails, the stale copy
/// is used so solves keep working offline.
#[tokio::test]
async fn test_cache_ttl_expired_falls_back_to_stale_copy() {
    setup_tracing();

    let mapping_url = "https://example.invalid/mapping.json";
    // A zero TTL means the cached copy is always considered expired.
    let pixi = PixiControl::from_manifest(&manifest_with_ttl_mapping(mapping_url, "0s")).unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();

    let cache_file = ttl_cache_path_for(cache_dir.path(), mapping_url);
    fs_err::create_dir_all(cache_file.parent().unwrap()).unwrap();
    fs_err::write(&cache_file, r#"{ "pixi-something-new": "stale-but-used" }"#).unwrap();

    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let package = packages.pop().unwrap();
    let purl = package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .unwrap();
    assert_eq!(purl.name(), "stale-but-used");
}

/// Without any cached copy, a failing fetch of a TTL-cached mapping is a hard
/// error.
#[tokio::test]
async fn test_cache_ttl_no_cache_and_fetch_failure_errors() {
    setup_tracing();

    let mapping_url = "https://example.invalid/mapping.json";
    let pixi = PixiControl::from_manifest(&manifest_with_ttl_mapping(mapping_url, "1h")).unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    let result = mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await;

    assert!(
        result.is_err(),
        "an uncached TTL mapping with a failing fetch must error"
    );
}

/// A failing prefix.dev lookup must point firewall-restricted users at the
/// manifest options that avoid the network.
#[tokio::test]
async fn test_prefix_fetch_failure_error_mentions_escape_hatches() {
    setup_tracing();

    // No `conda-pypi-map` -> the default prefix.dev chain, which needs the
    // network to look up the record by hash.
    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-network-error"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("pixi-something-new")];
    let err = mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .expect_err("an offline prefix.dev lookup should fail");

    let rendered = format!("{err:?}");
    // Strip all whitespace before matching: miette wraps the help text at
    // arbitrary points, potentially splitting tokens across lines.
    let collapsed: String = rendered.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        collapsed.contains("mode=\"replace\"") && collapsed.contains("conda-pypi-map=false"),
        "the error should suggest the offline escape hatches, got: {rendered}"
    );
}

/// `conda-pypi-map = {}` is a soft-deprecated alias for
/// `conda-pypi-map = false`; both disable all mapping lookups while keeping
/// the conda-forge verbatim fallback.
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

    let blocked_client = ClientBuilder::from_client(client.client().clone())
        .with(blocking_middleware)
        .build();

    let boltons_package = Package::build("boltons", "2").finish();

    let boltons_repo_data_record = RepoDataRecord {
        identifier: boltons_package.identifier(),
        package_record: boltons_package.package_record,
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mut packages = vec![boltons_repo_data_record];

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        blocked_client.into(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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

/// `conda-pypi-map = false` is the canonical global disable: no lookups, but
/// the conda-forge verbatim fallback still applies.
#[tokio::test]
async fn test_disabled_mapping_via_false() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-disable-false"
    channels = ["https://prefix.dev/conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = false
    "#,
    )
    .unwrap();

    let project = pixi.workspace().unwrap();
    let cache_dir = TempDir::new().unwrap();
    let mapping_client = offline_mapping_client(&project, cache_dir.path().to_path_buf());

    let mut packages = vec![conda_forge_record("boltons")];
    mapping_client
        .amend_purls(
            project.pypi_name_derivation_mode().unwrap(),
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
    assert_eq!(boltons_first_purl.name(), "boltons");
    assert!(boltons_first_purl.qualifiers().is_empty());
}

#[tokio::test]
async fn test_custom_mapping_ignores_backwards_compatibility() {
    setup_tracing();

    // Create local conda channel with boltons and python packages
    let mut package_database = MockRepoData::default();
    package_database.add_package(
        Package::build("python", "3.12.0")
            .with_subdir(Platform::Linux64)
            .finish(),
    );
    package_database.add_package(
        Package::build("boltons", "24.0.0")
            .with_subdir(Platform::Linux64)
            .finish(),
    );
    let channel = package_database.into_channel().await.unwrap();
    let channel_url = channel.url();

    // Create local PyPI index with boltons package
    let pypi_index = PyPIDatabase::new()
        .with(PyPIPackage::new("boltons", "24.0.0"))
        .into_simple_index()
        .expect("failed to create local simple index");

    // Create a project-defined mapping file that only includes specific packages
    let temp_dir = TempDir::new().unwrap();
    let mapping_file = temp_dir.path().join("map.json");
    fs_err::write(&mapping_file, r#"{}"#).unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [workspace]
    name = "test-custom-mapping"
    channels = ["{channel_url}"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ "{channel_url}" = {{ location = "{mapping_file}", mode = "replace" }} }}

    [dependencies]
    python = "3.12.0"
    boltons = "*"

    [pypi-dependencies]
    boltons = "*"

    [pypi-options]
    index-url = "{pypi_url}"
    "#,
        channel_url = channel_url,
        mapping_file = mapping_file
            .to_str()
            .unwrap()
            .to_string()
            .replace("\\", "/"),
        pypi_url = pypi_index.index_url(),
    ))
    .unwrap();

    // Lock the project (this triggers the amend_purls logic)
    pixi.lock().await.unwrap();

    // Get the lock file
    let lock = pixi.lock_file().await.unwrap();
    let p = lock.platform(&Platform::Linux64.to_string()).unwrap();
    let environment = lock.environment(DEFAULT_ENVIRONMENT_NAME).unwrap();
    let conda_packages = environment.conda_packages(p).unwrap();

    // Collect conda packages to a vector so we can iterate over them
    let conda_packages: Vec<_> = conda_packages.collect();

    // Find boltons in conda packages
    let boltons_package = conda_packages
        .iter()
        .find(|pkg| match pkg {
            rattler_lock::CondaPackageData::Binary(binary) => {
                binary.package_record.name.as_source() == "boltons"
            }
            _ => panic!("All packagees should be binary"),
        })
        .expect("boltons should be present in conda packages");

    // The issue: boltons should NOT have purls when using project-defined mapping
    // because it's not specified in our project-defined mapping
    // But due to backwards compatibility logic, it gets purls anyway
    let purls = match boltons_package {
        rattler_lock::CondaPackageData::Binary(binary) => &binary.package_record.purls,
        _ => panic!("All packages should be binary"),
    };

    assert!(
        purls.as_ref().is_none_or(|purls| purls.is_empty()),
        "boltons should not have purls when not specified in custom conda-pypi-map"
    );
}

#[tokio::test]
async fn test_missing_mapping_file_error_includes_path() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();

    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();

    // Use a non-existent file path for the project-defined mapping
    let non_existent_path = Path::new("/this/path/does/not/exist/mapping.json");

    let source = HashMap::from([(
        "https://conda.anaconda.org/conda-forge".to_owned(),
        ProjectDefinedChannelMapping::replace(ProjectDefinedMappingLocation::Path(
            non_existent_path.to_path_buf(),
        )),
    )]);

    let foo_bar_package = Package::build("foo-bar-car", "2").finish();

    let mut repo_data_record = RepoDataRecord {
        identifier: foo_bar_package.identifier(),
        package_record: foo_bar_package.package_record,
        url: Url::parse("https://pypi.org/simple/boltons/").unwrap(),
        channel: Some("https://conda.anaconda.org/conda-forge/".to_owned()),
    };

    let mapping_client = pypi_mapping::PurlDerivationClient::builder(
        client.clone(),
        project
            .config()
            .cache_dir_for(pixi_config::CacheKind::PypiMapping)
            .unwrap(),
    )
    .finish();
    let result = mapping_client
        .amend_purls(
            &PurlDerivationMode::ProjectDefined(Arc::new(ProjectDefinedMapping::new(source))),
            vec![&mut repo_data_record],
            None,
        )
        .await;

    // The operation should fail because the mapping file doesn't exist
    let err = result.expect_err("Expected an error when mapping file doesn't exist");
    insta::with_settings!({filters => vec![
        (r#"path: "([^"]+)""#, "[MAPPING_PATH]"),
        (r#"message: "[^"]+""#, "[MAPPING_MESSAGE]"),
        (r#"\bcode:\s*\d+\b"#, "[MAPPING_CODE]"),
    ]}, {
        insta::assert_debug_snapshot!(err);
    });
}
