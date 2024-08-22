use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    future::ready,
    rc::Rc,
};

use distribution_filename::SourceDistExtension;
use distribution_types::{
    Dist, File, FileLocation, HashComparison, IndexLocations, IndexUrl, PrioritizedDist,
    RegistrySourceDist, SourceDist, SourceDistCompatibility, UrlString,
};
use futures::{Future, FutureExt};
use pep508_rs::{PackageName, VerbatimUrl};
use pixi_consts::consts;
use rattler_conda_types::RepoDataRecord;
use uv_distribution::{ArchiveMetadata, Metadata};
use uv_resolver::{
    DefaultResolverProvider, MetadataResponse, ResolverProvider, VersionMap, VersionsResponse,
    WheelMetadataResult,
};
use uv_types::BuildContext;

use crate::lock_file::{records_by_name::HasNameVersion, PypiPackageIdentifier};

pub(super) struct CondaResolverProvider<'a, Context: BuildContext> {
    pub(super) fallback: DefaultResolverProvider<'a, Context>,
    pub(super) conda_python_identifiers:
        &'a HashMap<PackageName, (RepoDataRecord, PypiPackageIdentifier)>,

    /// Saves the number of requests by the uv solver per package
    pub(super) package_requests: Rc<RefCell<HashMap<PackageName, u32>>>,
}

impl<'a, Context: BuildContext> ResolverProvider for CondaResolverProvider<'a, Context> {
    fn get_package_versions<'io>(
        &'io self,
        package_name: &'io PackageName,
    ) -> impl Future<Output = uv_resolver::PackageVersionsResult> + 'io {
        if let Some((repodata_record, identifier)) = self.conda_python_identifiers.get(package_name)
        {
            // If we encounter a package that was installed by conda we simply return a single
            // available version in the form of a source distribution with the URL of the
            // conda package.
            //
            // Obviously this is not a valid source distribution but it eases debugging.

            // Don't think this matters much
            // so just fill it up with empty fields
            let file = File {
                dist_info_metadata: false,
                filename: identifier.name.as_normalized().clone().to_string(),
                hashes: vec![],
                requires_python: None,
                size: None,
                upload_time_utc_ms: None,
                url: FileLocation::AbsoluteUrl(UrlString::from(repodata_record.url.clone())),
                yanked: None,
            };

            let source_dist = RegistrySourceDist {
                name: identifier.name.as_normalized().clone(),
                version: repodata_record
                    .version()
                    .to_string()
                    .parse()
                    .expect("could not convert to pypi version"),
                file: Box::new(file),
                index: IndexUrl::Pypi(VerbatimUrl::from_url(
                    consts::DEFAULT_PYPI_INDEX_URL.clone(),
                )),
                wheels: vec![],
                ext: SourceDistExtension::TarGz,
            };

            let prioritized_dist = PrioritizedDist::from_source(
                source_dist,
                Vec::new(),
                SourceDistCompatibility::Compatible(HashComparison::Matched),
            );

            // Record that we got a request for this package so we can track the number of requests
            self.package_requests
                .borrow_mut()
                .entry(package_name.clone())
                .and_modify(|e| *e += 1)
                .or_insert(1);

            return ready(Ok(VersionsResponse::Found(vec![VersionMap::from(
                BTreeMap::from_iter([(identifier.version.clone(), prioritized_dist)]),
            )])))
            .right_future();
        }

        // Otherwise use the default implementation
        self.fallback
            .get_package_versions(package_name)
            .left_future()
    }

    fn get_or_build_wheel_metadata<'io>(
        &'io self,
        dist: &'io Dist,
    ) -> impl Future<Output = WheelMetadataResult> + 'io {
        if let Dist::Source(SourceDist::Registry(RegistrySourceDist { name, .. })) = dist {
            if let Some((_, iden)) = self.conda_python_identifiers.get(name) {
                // If this is a Source dist and the package is actually installed by conda we
                // create fake metadata with no dependencies. We assume that all conda installed
                // packages are properly installed including its dependencies.
                return ready(Ok(MetadataResponse::Found(ArchiveMetadata {
                    metadata: Metadata {
                        name: iden.name.as_normalized().clone(),
                        version: iden.version.clone(),
                        requires_dist: vec![],
                        requires_python: None,
                        provides_extras: iden.extras.iter().cloned().collect(),
                        dev_dependencies: Default::default(),
                    },
                    hashes: vec![],
                })))
                .left_future();
            }
        }

        // Otherwise just call the default implementation
        self.fallback
            .get_or_build_wheel_metadata(dist)
            .right_future()
    }

    fn index_locations(&self) -> &IndexLocations {
        self.fallback.index_locations()
    }

    fn with_reporter(self, reporter: impl uv_distribution::Reporter + 'static) -> Self {
        Self {
            fallback: self.fallback.with_reporter(reporter),
            ..self
        }
    }
}
