use std::{
    collections::{BTreeMap, HashMap},
    future::ready,
};

use distribution_types::{
    DirectUrlSourceDist, Dist, IndexLocations, PrioritizedDist, SourceDist, SourceDistCompatibility,
};
use futures::{Future, FutureExt};
use pep508_rs::{PackageName, VerbatimUrl};
use pypi_types::Metadata23;
use rattler_conda_types::RepoDataRecord;
use uv_distribution::ArchiveMetadata;
use uv_resolver::{
    DefaultResolverProvider, MetadataResponse, ResolverProvider, VersionMap, VersionsResponse,
    WheelMetadataResult,
};
use uv_types::BuildContext;

use crate::lock_file::PypiPackageIdentifier;

pub(super) struct CondaResolverProvider<'a, Context: BuildContext + Send + Sync> {
    pub(super) fallback: DefaultResolverProvider<'a, Context>,
    pub(super) conda_python_identifiers:
        &'a HashMap<PackageName, (RepoDataRecord, PypiPackageIdentifier)>,
}

impl<'a, Context: BuildContext + Send + Sync> ResolverProvider
    for CondaResolverProvider<'a, Context>
{
    fn get_package_versions<'io>(
        &'io self,
        package_name: &'io PackageName,
    ) -> impl Future<Output = uv_resolver::PackageVersionsResult> + Send + 'io {
        if let Some((repodata_record, identifier)) = self.conda_python_identifiers.get(package_name)
        {
            // If we encounter a package that was installed by conda we simply return a single
            // available version in the form of a source distribution with the URL of the
            // conda package.
            //
            // Obviously this is not a valid source distribution but it easies debugging.
            let dist = Dist::Source(SourceDist::DirectUrl(DirectUrlSourceDist {
                name: identifier.name.as_normalized().clone(),
                url: VerbatimUrl::unknown(repodata_record.url.clone()),
            }));

            let prioritized_dist = PrioritizedDist::from_source(
                dist,
                Vec::new(),
                SourceDistCompatibility::Compatible(distribution_types::Hash::Matched),
            );

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
    ) -> impl Future<Output = WheelMetadataResult> + Send + 'io {
        if let Dist::Source(SourceDist::DirectUrl(DirectUrlSourceDist { name, .. })) = dist {
            if let Some((_, iden)) = self.conda_python_identifiers.get(name) {
                // If this is a Source dist and the package is actually installed by conda we
                // create fake metadata with no dependencies. We assume that all conda installed
                // packages are properly installed including its dependencies.
                return ready(Ok(MetadataResponse::Found(ArchiveMetadata {
                    metadata: Metadata23 {
                        name: iden.name.as_normalized().clone(),
                        version: iden.version.clone(),
                        requires_dist: vec![],
                        requires_python: None,
                        provides_extras: iden.extras.iter().cloned().collect(),
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
