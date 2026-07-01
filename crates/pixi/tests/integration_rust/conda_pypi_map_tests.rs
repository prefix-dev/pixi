//! Tests for the conda↔PyPI name mapping: purl derivation through the
//! prefix.dev chain, project-defined `conda-pypi-map` overrides in their
//! overlay/replace/disabled modes, and the offline stale-fallback cache for
//! remote mapping locations.

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
    // Add and update lock file with this version of python.
    // Pin to a version that is present in the prefix.dev hash mapping so the
    // purl is derived from the hash mapping rather than the compressed mapping.
    pixi.add("boltons ==25.0.0").await.unwrap();
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
    pixi.add("boltons ==25.0.0")
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
    conda-pypi-map = {{ 'conda-forge' = {{ location = "{}", mapping-mode = "replace", same-name-heuristic = false }} }}
    "#,
        absolute_compressed_mapping_path()
    ))
    .unwrap();

    let project = pixi.workspace().unwrap();

    let client = project.authenticated_client().unwrap();

    // We use one package that is present in our mapping: `boltons`
    // and another one that is missing from conda and our mapping:
    // `pixi-something-new-for-test`. Because the mapping uses
    // `mapping-mode = "replace"` skips Pixi's default mapping data, and
    // `same-name-heuristic = false` also disables the same-name heuristic.
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

    // for boltons we have a mapping record
    // so we test that we also record source=project-defined-mapping qualifier
    assert_eq!(boltons_first_purl.name(), "boltons");
    assert_eq!(
        boltons_first_purl.qualifiers().get("source").unwrap(),
        PurlDerivationSource::ProjectDefinedMapping.as_str()
    );

    let package = packages.pop().unwrap();

    // With replacement mapping data and the same-name heuristic disabled,
    // packages not in the project-defined mapping should NOT get purls.
    assert!(
        package.package_record.purls.is_none()
            || package.package_record.purls.as_ref().unwrap().is_empty(),
        "pixi-something-new should not have purls when not in a replace-mapping-mode mapping"
    );
}

#[tokio::test]
async fn test_replace_mapping_mode_keeps_same_name_heuristic_by_default() {
    setup_tracing();

    let temp_dir = TempDir::new().unwrap();
    let mapping_file = temp_dir.path().join("empty-map.json");
    fs_err::write(&mapping_file, r#"{}"#).unwrap();

    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-replace-keeps-same-name"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ conda-forge = {{ location = "{}", mapping-mode = "replace" }} }}
    "#,
        mapping_file.display().to_string().replace("\\", "/")
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
        .expect("same-name heuristic should still run after replacement mapping data misses");
    assert_eq!(purl.name(), "pixi-something-new");
    assert!(purl.qualifiers().is_empty());
}

#[tokio::test]
async fn test_same_name_heuristic_can_be_enabled_for_any_channel() {
    setup_tracing();

    let pixi = PixiControl::new().unwrap();
    pixi.init().await.unwrap();
    let project = pixi.workspace().unwrap();
    let client = project.authenticated_client().unwrap();

    let foo_package = Package::build("my-internal-package", "2").finish();
    let mut repo_data_record = RepoDataRecord {
        identifier: foo_package.identifier(),
        package_record: foo_package.package_record,
        url: Url::parse("https://example.com/my-internal-package").unwrap(),
        channel: Some("internal-channel".to_owned()),
    };

    let source = HashMap::from([(
        "internal-channel".to_owned(),
        ProjectDefinedChannelMapping::new(Vec::new(), pypi_mapping::MappingMode::Replace, true),
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

    let purl = repo_data_record
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .expect("same-name heuristic should be usable for explicitly configured channels");
    assert_eq!(purl.name(), "my-internal-package");
    assert!(purl.qualifiers().is_empty());
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

/// An inline mapping hit in the default (overlay) mode is final and requires
/// no network access.
#[tokio::test]
async fn test_overlay_mapping_inline_hit_without_network() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-overlay-inline"
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
/// derived and the same-name heuristic does not kick in either.
#[tokio::test]
async fn test_overlay_mapping_explicit_false_yields_no_purl() {
    setup_tracing();

    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-overlay-false"
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

/// `<channel> = false` disables purl derivation for that channel, including
/// the offline same-name heuristic.
#[tokio::test]
async fn test_channel_disabled_suppresses_same_name_heuristic() {
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
    assert!(
        package
            .package_record
            .purls
            .as_ref()
            .is_some_and(|purls| purls.is_empty()),
        "channel disable should write empty purls to suppress compatibility same-name logic"
    );
    // Downstream PyPI resolution treats `purls = None` as an old lock file and
    // applies compatibility same-name logic for conda-forge records. Writing
    // `Some(empty)` records that this package is known not to satisfy PyPI names.
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
    conda-pypi-map = {{ conda-forge = {{ location = "{mapping_url}", mapping-mode = "overlay" }} }}
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

/// In overlay mode a miss in the project-defined mapping falls through to the
/// prefix.dev chain.
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_overlay_mapping_miss_falls_through_to_prefix() {
    setup_tracing();

    // The mapping contains an unrelated package, so `boltons` is a miss and
    // must be resolved through the prefix.dev chain (the mock-built record's
    // hash is unknown there, so the compressed name mapping answers).
    let pixi = PixiControl::from_manifest(
        r#"
    [project]
    name = "test-overlay-miss"
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
/// they go through the full default chain, including the conda-forge same-name
/// fallback.
///
/// This pins the per-record fallback behavior: previously, configuring any
/// `conda-pypi-map` suppressed the same-name heuristic globally, degrading purl
/// coverage even for channels that were not in the map.
#[tokio::test]
#[cfg_attr(not(feature = "online_tests"), ignore)]
async fn test_mapping_for_other_channel_keeps_same_name_heuristic() {
    setup_tracing();

    // A replace-mapping-mode mapping for robostack only; the record below comes from
    // conda-forge and its name is unknown to both the prefix.dev hash and
    // compressed mappings, so only the same-name heuristic can answer.
    let pixi = PixiControl::from_manifest(&format!(
        r#"
    [project]
    name = "test-unmapped-channel-same-name"
    channels = ["conda-forge", "robostack"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ robostack = {{ location = "{}", mapping-mode = "replace" }} }}
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
        .expect("a record from an unmapped channel should get the same-name heuristic purl");
    // The same-name heuristic assumes the conda name is the pypi name and adds
    // no source qualifier.
    assert_eq!(purl.name(), "pixi-something-new");
    assert!(purl.qualifiers().is_empty());
}

fn manifest_with_remote_mapping(url: &str) -> String {
    format!(
        r#"
    [project]
    name = "test-mapping-cache"
    channels = ["conda-forge"]
    platforms = ["linux-64"]
    conda-pypi-map = {{ conda-forge = {{ location = "{url}" }} }}
    "#
    )
}

/// A `PurlDerivationClient` that uses the project's real (online) client and
/// the given shared cache dir, so a fetch populates the on-disk HTTP cache.
fn online_mapping_client(
    project: &pixi_core::Workspace,
    cache_dir: std::path::PathBuf,
) -> pypi_mapping::PurlDerivationClient {
    let client = project.authenticated_client().unwrap().clone();
    pypi_mapping::PurlDerivationClient::builder(client, cache_dir).finish()
}

/// Start a minimal localhost HTTP server that serves the conda-pypi mapping
/// with a cacheable response (`Cache-Control: max-age=3600, public`). The first
/// request gets `first_body`, every later request gets `later_body`, so a test
/// can tell whether a second fetch came from cache or from the network.
fn serve_cacheable_mapping(first_body: &'static str, later_body: &'static str) -> String {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let count = AtomicUsize::new(0);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // Read (and ignore) the request headers.
            let _ = stream.read(&mut [0u8; 1024]);
            let body = if count.fetch_add(1, Ordering::SeqCst) == 0 {
                first_body
            } else {
                later_body
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                 Cache-Control: max-age=3600, public\r\nETag: \"v1\"\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    format!("http://{addr}/mapping.json")
}

/// A remote mapping that has been fetched once is served from the on-disk HTTP
/// cache on a later solve without touching the network — so a solve keeps
/// working offline. Caching is handled entirely by the `http-cache` middleware
/// (`CacheMode::Default`) using the server's `Cache-Control`; we no longer keep
/// a separate copy.
#[tokio::test]
async fn test_remote_mapping_reused_from_cache_offline() {
    setup_tracing();

    let mapping_url = serve_cacheable_mapping(
        r#"{ "pixi-something-new": "from-cache" }"#,
        r#"{ "pixi-something-new": "live-second" }"#,
    );
    let manifest = manifest_with_remote_mapping(&mapping_url);
    // A cache dir shared by both clients; the second client is a fresh project
    // so its in-memory mapping cache is empty and it must consult the HTTP
    // cache on disk.
    let cache_dir = TempDir::new().unwrap();

    // 1. Seed the on-disk HTTP cache with one online fetch.
    let seeding = PixiControl::from_manifest(&manifest).unwrap();
    let seeding_project = seeding.workspace().unwrap();
    let online = online_mapping_client(&seeding_project, cache_dir.path().to_path_buf());
    let mut packages = vec![conda_forge_record("pixi-something-new")];
    online
        .amend_purls(
            seeding_project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        packages[0]
            .package_record
            .purls
            .as_ref()
            .and_then(BTreeSet::first)
            .unwrap()
            .name(),
        "from-cache"
    );

    drop(online);

    // 2. A second, independent project sharing the cache dir resolves the same
    // mapping. It must be served from the on-disk HTTP cache without hitting
    // the network: if it did reach the server it would get `live-second`
    // instead of the cached `from-cache`. This is the offline-tolerance
    // guarantee — a freshly cached mapping needs no network on later solves.
    let second_pixi = PixiControl::from_manifest(&manifest).unwrap();
    let second_project = second_pixi.workspace().unwrap();
    let second = online_mapping_client(&second_project, cache_dir.path().to_path_buf());
    let mut packages = vec![conda_forge_record("pixi-something-new")];
    second
        .amend_purls(
            second_project.pypi_name_derivation_mode().unwrap(),
            &mut packages,
            None,
        )
        .await
        .unwrap();

    let purl = packages
        .pop()
        .unwrap()
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .cloned()
        .unwrap();
    assert_eq!(
        purl.name(),
        "from-cache",
        "the second solve must reuse the cached mapping, not re-fetch it"
    );
}

/// Without any cached copy, a failing fetch of a remote mapping is a hard
/// error.
#[tokio::test]
async fn test_remote_mapping_no_cache_and_fetch_failure_errors() {
    setup_tracing();

    let mapping_url = "https://example.invalid/mapping.json";
    let pixi = PixiControl::from_manifest(&manifest_with_remote_mapping(mapping_url)).unwrap();

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
        "an uncached remote mapping with a failing fetch must error"
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
        collapsed.contains("mapping-mode=\"replace\"")
            && collapsed.contains("conda-pypi-map=false"),
        "the error should suggest the offline escape hatches, got: {rendered}"
    );
}

/// `conda-pypi-map = {}` is a soft-deprecated legacy spelling for avoiding
/// default mapping lookups while keeping the conda-forge same-name heuristic.
#[tokio::test]
async fn test_empty_mapping_keeps_legacy_same_name_heuristic() {
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

    let purl = boltons_package
        .package_record
        .purls
        .as_ref()
        .and_then(BTreeSet::first)
        .expect("empty conda-pypi-map should preserve the legacy same-name heuristic");
    assert_eq!(purl.name(), "boltons");
    assert!(purl.qualifiers().is_empty());
}

/// `conda-pypi-map = false` is the canonical global disable: no purl
/// derivation, including the same-name heuristic.
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
    assert!(
        boltons_package
            .package_record
            .purls
            .as_ref()
            .and_then(BTreeSet::first)
            .is_none(),
        "global disable should suppress the same-name heuristic"
    );
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
    conda-pypi-map = {{ "{channel_url}" = {{ location = "{mapping_file}", mapping-mode = "replace" }} }}

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
