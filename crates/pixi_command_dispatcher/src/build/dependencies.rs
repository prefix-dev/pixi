use std::str::FromStr;
use std::sync::Arc;

use itertools::Either;
use pixi_build_types::{
    BinaryPackageSpecV1, NamedSpecV1, PackageSpecV1,
    procedures::conda_outputs::{
        CondaOutputDependencies, CondaOutputIgnoreRunExports, CondaOutputRunExports,
    },
};
use pixi_record::PixiRecord;
use pixi_spec::{BinarySpec, DetailedSpec, PixiSpec, SourceAnchor, UrlBinarySpec};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    InvalidPackageNameError, MatchSpec, NamedChannelOrUrl, NamelessMatchSpec, PackageName,
    ParseStrictness, Platform, VersionSpec,
};
use rattler_repodata_gateway::{Gateway, RunExportExtractorError, RunExportsReporter};

use super::conversion;
use crate::SourceMetadataError;

#[derive(Debug)]
pub enum DependenciesError {
    InvalidPackageName(String, InvalidPackageNameError),
}

#[derive(Debug, Clone, Default)]
pub struct Dependencies {
    pub dependencies: DependencyMap<rattler_conda_types::PackageName, PixiSpec>,
    pub constraints: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,
}

impl Dependencies {
    pub fn new(
        output: &CondaOutputDependencies,
        source_anchor: Option<SourceAnchor>,
    ) -> Result<Self, DependenciesError> {
        let mut dependencies = DependencyMap::default();
        let mut constraints = DependencyMap::default();

        for depend in &output.depends {
            let name = rattler_conda_types::PackageName::from_str(&depend.name).map_err(|err| {
                DependenciesError::InvalidPackageName(depend.name.to_owned(), err)
            })?;
            match conversion::from_package_spec_v1(depend.spec.clone()).into_source_or_binary() {
                Either::Left(source) => {
                    let source = if let Some(anchor) = &source_anchor {
                        anchor.resolve(source)
                    } else {
                        source
                    };
                    dependencies.insert(name, PixiSpec::from(source));
                }
                Either::Right(binary) => {
                    dependencies.insert(name, PixiSpec::from(binary));
                }
            }
        }

        for constraint in &output.constraints {
            let name =
                rattler_conda_types::PackageName::from_str(&constraint.name).map_err(|err| {
                    DependenciesError::InvalidPackageName(constraint.name.to_owned(), err)
                })?;
            constraints.insert(
                name,
                conversion::from_binary_spec_v1(constraint.spec.clone()),
            );
        }

        Ok(Self {
            dependencies,
            constraints,
        })
    }

    pub fn extend_with_run_exports_from_build(
        mut self,
        build_run_exports: &PixiRunExports,
    ) -> Self {
        for (name, spec) in build_run_exports.strong.iter_specs() {
            self.dependencies.insert(name.clone(), spec.clone());
        }

        for (name, spec) in build_run_exports.strong_constrains.iter_specs() {
            self.constraints.insert(name.clone(), spec.clone());
        }

        self
    }

    /// Adds a set of dependencies to the current dependencies.
    fn add_dependencies(&mut self, specs: DependencyMap<PackageName, PixiSpec>) {
        self.dependencies.extend(specs.into_specs());
    }

    /// Adds a set of constraints to the current constraints.
    fn add_constraints(&mut self, specs: DependencyMap<PackageName, BinarySpec>) {
        self.constraints.extend(specs.into_specs())
    }

    pub fn extend_with_run_exports_from_build_and_host(
        mut self,
        host_run_exports: PixiRunExports,
        build_run_exports: PixiRunExports,
        target_platform: Platform,
    ) -> Self {
        if target_platform == Platform::NoArch {
            self.add_dependencies(host_run_exports.noarch);
        } else {
            self.add_dependencies(build_run_exports.strong);
            self.add_dependencies(host_run_exports.strong);
            self.add_dependencies(host_run_exports.weak);
            self.add_constraints(build_run_exports.strong_constrains);
            self.add_constraints(host_run_exports.strong_constrains);
            self.add_constraints(host_run_exports.weak_constrains);
        }

        self
    }

    /// Extract run exports from the solved environments.
    pub async fn extract_run_exports(
        &self,
        records: &mut [PixiRecord],
        ignore: &CondaOutputIgnoreRunExports,
        gateway: &Gateway,
        reporter: Option<Arc<dyn RunExportsReporter>>,
    ) -> Result<PixiRunExports, RunExportExtractorError> {
        let mut filter_run_exports = PixiRunExports::default();

        fn filter_match_specs<T: From<BinarySpec>>(
            specs: &[String],
            ignore: &CondaOutputIgnoreRunExports,
        ) -> Vec<(PackageName, T)> {
            specs
                .iter()
                .filter_map(move |spec| {
                    let (Some(name), spec) = MatchSpec::from_str(spec, ParseStrictness::Lenient)
                        .ok()?
                        .into_nameless()
                    else {
                        return None;
                    };
                    if ignore.by_name.contains(&name) {
                        return None;
                    }

                    let binary_spec = match spec {
                        NamelessMatchSpec {
                            url: Some(url),
                            sha256,
                            md5,
                            ..
                        } => BinarySpec::Url(UrlBinarySpec { url, sha256, md5 }),
                        NamelessMatchSpec {
                            version,
                            build: None,
                            build_number: None,
                            file_name: None,
                            extras: None,
                            channel: None,
                            subdir: None,
                            namespace: None,
                            md5: None,
                            sha256: None,
                            url: _,
                            license: None,
                        } => BinarySpec::Version(version.unwrap_or(VersionSpec::Any)),
                        NamelessMatchSpec {
                            version,
                            build,
                            build_number,
                            file_name,
                            channel,
                            subdir,
                            md5,
                            sha256,
                            license,

                            // Caught in the above case
                            url: _,

                            // Explicitly ignored
                            namespace: _,
                            extras: _,
                        } => BinarySpec::DetailedVersion(Box::new(DetailedSpec {
                            version,
                            build,
                            build_number,
                            file_name,
                            channel: channel
                                .map(|c| NamedChannelOrUrl::Url(c.base_url.clone().into())),
                            subdir,
                            md5,
                            sha256,
                            license,
                        })),
                    };

                    Some((name, binary_spec.into()))
                })
                .collect()
        }

        let mut filtered_records = records
            .iter_mut()
            // Only record run exports for packages that are direct dependencies.
            .filter(|r| self.dependencies.contains_key(&r.package_record().name))
            // Filter based on whether we want to ignore run exports for a particular
            // package.
            .filter(|r| !ignore.from_package.contains(&r.package_record().name))
            .collect::<Vec<_>>();

        let repodata_records = filtered_records.iter_mut().flat_map(|r| match *r {
            PixiRecord::Binary(repo_data_record) => Some(repo_data_record),
            PixiRecord::Source(_source_record) => None,
        });
        tracing::debug!("Making sure that run exports in pixi records");
        gateway
            .ensure_run_exports(repodata_records, reporter)
            .await?;

        for record in filtered_records {
            // Make sure we have valid run exports.
            let Some(run_exports) = &record.package_record().run_exports else {
                unreachable!(
                    "We tried to make sure that run exports are available but something went wrong"
                );
            };

            filter_run_exports
                .noarch
                .extend(filter_match_specs(&run_exports.noarch, ignore));
            filter_run_exports
                .strong
                .extend(filter_match_specs(&run_exports.strong, ignore));
            filter_run_exports
                .strong_constrains
                .extend(filter_match_specs(&run_exports.strong_constrains, ignore));
            filter_run_exports
                .weak
                .extend(filter_match_specs(&run_exports.weak, ignore));
            filter_run_exports
                .weak_constrains
                .extend(filter_match_specs(&run_exports.weak_constrains, ignore));
        }

        Ok(filter_run_exports)
    }
}

/// A variant of [`rattler_conda_types::package::RunExportsJson`] but with pixi
/// data types.
#[derive(Debug, Default, Clone)]
pub struct PixiRunExports {
    pub noarch: DependencyMap<PackageName, PixiSpec>,
    pub strong: DependencyMap<PackageName, PixiSpec>,
    pub weak: DependencyMap<PackageName, PixiSpec>,

    pub strong_constrains: DependencyMap<PackageName, BinarySpec>,
    pub weak_constrains: DependencyMap<PackageName, BinarySpec>,
}

impl PixiRunExports {
    /// Converts a [`CondaOutputRunExports`] to a [`PixiRunExports`].
    pub fn try_from_protocol(output: &CondaOutputRunExports) -> Result<Self, SourceMetadataError> {
        fn convert_package_spec(
            specs: &[NamedSpecV1<PackageSpecV1>],
        ) -> Result<DependencyMap<PackageName, PixiSpec>, SourceMetadataError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let spec = conversion::from_package_spec_v1(named_spec.spec);
                    let name = PackageName::from_str(&named_spec.name).map_err(|err| {
                        SourceMetadataError::InvalidPackageName(named_spec.name.to_owned(), err)
                    })?;
                    Ok((name, spec))
                })
                .collect()
        }

        fn convert_binary_spec(
            specs: &[NamedSpecV1<BinaryPackageSpecV1>],
        ) -> Result<DependencyMap<PackageName, BinarySpec>, SourceMetadataError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let spec = conversion::from_binary_spec_v1(named_spec.spec);
                    let name = PackageName::from_str(&named_spec.name).map_err(|err| {
                        SourceMetadataError::InvalidPackageName(named_spec.name.to_owned(), err)
                    })?;
                    Ok((name, spec))
                })
                .collect()
        }

        Ok(PixiRunExports {
            weak: convert_package_spec(&output.weak)?,
            strong: convert_package_spec(&output.strong)?,
            noarch: convert_package_spec(&output.noarch)?,
            weak_constrains: convert_binary_spec(&output.weak_constrains)?,
            strong_constrains: convert_binary_spec(&output.strong_constrains)?,
        })
    }
}
