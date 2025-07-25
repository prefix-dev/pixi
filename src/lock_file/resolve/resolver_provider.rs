use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    future::ready,
    rc::Rc,
    str::FromStr,
    sync::Arc,
};

use futures::{Future, FutureExt};
use pixi_consts::consts;
use pixi_record::PixiRecord;
use uv_distribution::{ArchiveMetadata, Metadata};
use uv_distribution_filename::SourceDistExtension;
use uv_distribution_types::{
    Dist, File, FileLocation, HashComparison, IndexUrl, PrioritizedDist, RegistrySourceDist,
    SourceDist, SourceDistCompatibility, UrlString,
};
use uv_redacted::DisplaySafeUrl;
use uv_resolver::{
    DefaultResolverProvider, FlatDistributions, MetadataResponse, ResolverProvider, VersionMap,
    VersionsResponse, WheelMetadataResult,
};
use uv_types::BuildContext;

use crate::lock_file::PypiPackageIdentifier;

pub(super) struct CondaResolverProvider<'a, Context: BuildContext> {
    pub(super) fallback: DefaultResolverProvider<'a, Context>,
    pub(super) conda_python_identifiers:
        &'a HashMap<uv_normalize::PackageName, (PixiRecord, PypiPackageIdentifier)>,

    /// Saves the number of requests by the uv solver per package
    pub(super) package_requests: Rc<RefCell<HashMap<uv_normalize::PackageName, u32>>>,
}

impl<Context: BuildContext> ResolverProvider for CondaResolverProvider<'_, Context> {
    fn get_package_versions<'io>(
        &'io self,
        package_name: &'io uv_normalize::PackageName,
        index: Option<&'io uv_distribution_types::IndexMetadata>,
    ) -> impl Future<Output = uv_resolver::PackageVersionsResult> + 'io {
        if let Some((repodata_record, identifier)) = self.conda_python_identifiers.get(package_name)
        {
            let version = repodata_record.package_record().version.to_string();

            tracing::debug!(
                "overriding PyPI package version request {}=={}",
                package_name,
                version
            );
            // If we encounter a package that was installed by conda we simply return a
            // single available version in the form of a source distribution
            // with the URL of the conda package.
            //
            // Obviously this is not a valid source distribution but it eases debugging.

            // Don't think this matters much
            // so just fill it up with empty fields
            let file = File {
                dist_info_metadata: false,
                filename: identifier.name.as_normalized().as_ref().into(),
                hashes: vec![].into(),
                requires_python: None,
                size: None,
                upload_time_utc_ms: None,
                url: match repodata_record {
                    PixiRecord::Binary(repodata_record) => FileLocation::AbsoluteUrl(
                        UrlString::from(DisplaySafeUrl::from(repodata_record.url.clone())),
                    ),
                    PixiRecord::Source(_source) => {
                        // TODO(baszalmstra): Does this matter??
                        FileLocation::RelativeUrl("foo".into(), "bar".into())
                    }
                },
                yanked: None,
            };

            let source_dist = RegistrySourceDist {
                name: uv_normalize::PackageName::from_str(identifier.name.as_normalized().as_ref())
                    .expect("invalid package name"),
                version: version.parse().expect("could not convert to pypi version"),
                file: Box::new(file),
                index: IndexUrl::Pypi(Arc::new(uv_pep508::VerbatimUrl::from_url(
                    DisplaySafeUrl::from(consts::DEFAULT_PYPI_INDEX_URL.clone()),
                ))),
                wheels: vec![],
                ext: SourceDistExtension::TarGz,
            };

            let prioritized_dist = PrioritizedDist::from_source(
                source_dist,
                Vec::new(),
                SourceDistCompatibility::Compatible(HashComparison::Matched),
            );

            // Record that we got a request for this package so we can track the number of
            // requests
            self.package_requests
                .borrow_mut()
                .entry(package_name.clone())
                .and_modify(|e| *e += 1)
                .or_insert(1);

            // Convert version
            let version = identifier.version.to_string();
            let version =
                uv_pep440::Version::from_str(&version).expect("could not convert to pypi version");

            // TODO: very unsafe but we need to convert the BTreeMap to a FlatDistributions
            //       should make a PR to be able to set this directly
            let version_map = BTreeMap::from_iter([(version, prioritized_dist)]);
            let flat_dists = FlatDistributions::from(version_map);

            return ready(Ok(VersionsResponse::Found(vec![VersionMap::from(
                flat_dists,
            )])))
            .right_future();
        }

        // Otherwise use the default implementation
        self.fallback
            .get_package_versions(package_name, index)
            .left_future()
    }

    fn get_or_build_wheel_metadata<'io>(
        &'io self,
        dist: &'io Dist,
    ) -> impl Future<Output = WheelMetadataResult> + 'io {
        if let Dist::Source(SourceDist::Registry(RegistrySourceDist { name, .. })) = dist {
            if let Some((_, iden)) = self.conda_python_identifiers.get(name) {
                tracing::debug!("overriding PyPI package metadata request {}", name);
                // If this is a Source dist and the package is actually installed by conda we
                // create fake metadata with no dependencies. We assume that all conda installed
                // packages are properly installed including its dependencies.
                //
                let name = uv_normalize::PackageName::from_str(iden.name.as_normalized().as_ref())
                    .expect("invalid package name");
                let version = uv_pep440::Version::from_str(&iden.version.to_string())
                    .expect("could not convert to pypi version");
                return ready(Ok(MetadataResponse::Found(ArchiveMetadata {
                    metadata: Metadata {
                        name,
                        version,
                        requires_dist: vec![].into(),
                        requires_python: None,
                        provides_extras: iden.extras.iter().cloned().collect(),
                        dependency_groups: Default::default(),
                        dynamic: false,
                    },
                    hashes: vec![].into(),
                })))
                .left_future();
            }
        }

        // Otherwise just call the default implementation
        self.fallback
            .get_or_build_wheel_metadata(dist)
            .right_future()
    }

    fn with_reporter(self, reporter: Arc<dyn uv_distribution::Reporter>) -> Self {
        Self {
            fallback: self.fallback.with_reporter(reporter),
            ..self
        }
    }

    fn get_installed_metadata<'io>(
        &'io self,
        dist: &'io uv_distribution_types::InstalledDist,
    ) -> impl Future<Output = WheelMetadataResult> + 'io {
        self.fallback.get_installed_metadata(dist)
    }
}
