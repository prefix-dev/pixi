use std::{fmt::Display, hash::Hash, str::FromStr, sync::Arc};

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
use serde::Serialize;

use super::conversion;

#[derive(Debug)]
pub enum DependenciesError {
    InvalidPackageName(String, InvalidPackageNameError),
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Dependencies {
    pub dependencies: DependencyMap<rattler_conda_types::PackageName, WithSource<PixiSpec>>,
    pub constraints: DependencyMap<rattler_conda_types::PackageName, WithSource<BinarySpec>>,
}

#[derive(Debug, Clone, Serialize, Hash, Eq, PartialEq)]
pub enum KnownEnvironment {
    Build,
    Host,
}

impl Display for KnownEnvironment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KnownEnvironment::Build => write!(f, "build"),
            KnownEnvironment::Host => write!(f, "host"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Hash, Eq, PartialEq)]
pub enum DependencySource {
    RunExport {
        name: PackageName,
        env: KnownEnvironment,
    },
}

#[derive(Debug, Clone, Serialize, Hash, Eq, PartialEq)]
pub struct WithSource<T> {
    pub value: T,
    pub source: Option<DependencySource>,
}

impl<T> From<T> for WithSource<T> {
    fn from(value: T) -> Self {
        WithSource::new(value)
    }
}

impl<T> WithSource<T> {
    pub fn with_source(self, source: DependencySource) -> Self {
        Self {
            source: Some(source),
            ..self
        }
    }

    pub fn new(value: T) -> Self {
        Self {
            value,
            source: None,
        }
    }
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
                    dependencies.insert(name, PixiSpec::from(source).into());
                }
                Either::Right(binary) => {
                    dependencies.insert(name, PixiSpec::from(binary).into());
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
                conversion::from_binary_spec_v1(constraint.spec.clone()).into(),
            );
        }

        Ok(Self {
            dependencies,
            constraints,
        })
    }

    pub fn extend_with_run_exports_from_build(
        mut self,
        build_run_exports: &[(PackageName, PixiRunExports)],
    ) -> Self {
        for (package_name, run_exports) in build_run_exports {
            for (name, spec) in run_exports.strong.iter_specs() {
                self.dependencies.insert(
                    name.clone(),
                    WithSource::new(spec.clone()).with_source(DependencySource::RunExport {
                        name: package_name.clone(),
                        env: KnownEnvironment::Build,
                    }),
                );
            }

            for (name, spec) in run_exports.strong_constrains.iter_specs() {
                self.constraints.insert(
                    name.clone(),
                    WithSource::new(spec.clone()).with_source(DependencySource::RunExport {
                        name: package_name.clone(),
                        env: KnownEnvironment::Build,
                    }),
                );
            }
        }

        self
    }

    pub fn extend_with_run_exports_from_build_and_host(
        mut self,
        mut host_run_exports: Vec<(PackageName, PixiRunExports)>,
        mut build_run_exports: Vec<(PackageName, PixiRunExports)>,
        target_platform: Platform,
    ) -> Self {
        macro_rules! extend_with_run_exports {
            ($target:expr, $export_type:ident, Build) => {
                extend_with_run_exports!(
                    $target,
                    build_run_exports,
                    $export_type,
                    KnownEnvironment::Build
                );
            };
            ($target:expr, $export_type:ident, Host) => {
                extend_with_run_exports!(
                    $target,
                    host_run_exports,
                    $export_type,
                    KnownEnvironment::Host
                );
            };
            ($target:expr, $run_exports:expr, $export_type:ident, $env:expr) => {
                for (package_name, run_exports) in $run_exports.iter_mut() {
                    $target.extend(
                        std::mem::take(&mut run_exports.$export_type)
                            .into_specs()
                            .map(|(name, spec)| {
                                (
                                    name,
                                    WithSource::new(spec).with_source(
                                        DependencySource::RunExport {
                                            name: package_name.clone(),
                                            env: $env,
                                        },
                                    ),
                                )
                            }),
                    );
                }
            };
        }

        if target_platform == Platform::NoArch {
            extend_with_run_exports!(self.dependencies, noarch, Host);
        } else {
            extend_with_run_exports!(self.dependencies, strong, Host);
            extend_with_run_exports!(self.dependencies, strong, Build);
            extend_with_run_exports!(self.dependencies, weak, Host);
            extend_with_run_exports!(self.constraints, strong_constrains, Host);
            extend_with_run_exports!(self.constraints, strong_constrains, Build);
            extend_with_run_exports!(self.constraints, weak_constrains, Host);
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
    ) -> Result<Vec<(PackageName, PixiRunExports)>, RunExportExtractorError> {
        let mut combined_run_exports = Vec::new();

        // Find all the records that are relevant for run exports.
        let mut relevant_records = records
            .iter_mut()
            // Only record run exports for packages that are direct dependencies.
            .filter(|r| self.dependencies.contains_key(&r.package_record().name))
            // Filter based on whether we want to ignore run exports for a particular
            // package.
            .filter(|r| !ignore.from_package.contains(&r.package_record().name))
            .collect::<Vec<_>>();

        // Determine the records that have missing run exports.
        let records_missing_run_exports = relevant_records
            .iter_mut()
            .flat_map(|r| match *r {
                PixiRecord::Binary(repo_data_record) => Some(repo_data_record),
                PixiRecord::Source(_source_record) => None,
            })
            .filter(|r| r.package_record.run_exports.is_none());
        gateway
            .ensure_run_exports(records_missing_run_exports.into_iter(), reporter)
            .await?;

        for record in relevant_records {
            // Only record run exports for packages that are direct dependencies.
            if !self
                .dependencies
                .contains_key(&record.package_record().name)
            {
                continue;
            }

            // Filter based on whether we want to ignore run exports for a particular
            // package.
            if ignore.from_package.contains(&record.package_record().name) {
                continue;
            }

            // Make sure we have valid run exports.
            let Some(run_exports) = &record.package_record().run_exports else {
                // No run-exports found
                continue;
            };

            let filtered_run_exports = PixiRunExports {
                noarch: filter_match_specs(&run_exports.noarch, ignore),
                strong: filter_match_specs(&run_exports.strong, ignore),
                weak: filter_match_specs(&run_exports.weak, ignore),
                strong_constrains: filter_match_specs(&run_exports.strong_constrains, ignore),
                weak_constrains: filter_match_specs(&run_exports.weak_constrains, ignore),
            };

            combined_run_exports.push((record.name().clone(), filtered_run_exports));
        }

        Ok(combined_run_exports)
    }
}

fn filter_match_specs<T: From<BinarySpec> + Clone + Hash + Eq + PartialEq>(
    specs: &[String],
    ignore: &CondaOutputIgnoreRunExports,
) -> DependencyMap<PackageName, T> {
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
                    channel: channel.map(|c| NamedChannelOrUrl::Url(c.base_url.clone().into())),
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

/// A variant of [`rattler_conda_types::package::RunExportsJson`] but with pixi
/// data types.
#[derive(Debug, Serialize, Default, Clone)]
pub struct PixiRunExports {
    pub noarch: DependencyMap<PackageName, PixiSpec>,
    pub strong: DependencyMap<PackageName, PixiSpec>,
    pub weak: DependencyMap<PackageName, PixiSpec>,

    pub strong_constrains: DependencyMap<PackageName, BinarySpec>,
    pub weak_constrains: DependencyMap<PackageName, BinarySpec>,
}

impl PixiRunExports {
    /// Converts a [`CondaOutputRunExports`] to a [`PixiRunExports`].
    pub fn try_from_protocol(output: &CondaOutputRunExports) -> Result<Self, DependenciesError> {
        fn convert_package_spec(
            specs: &[NamedSpecV1<PackageSpecV1>],
        ) -> Result<DependencyMap<PackageName, PixiSpec>, DependenciesError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let spec = conversion::from_package_spec_v1(named_spec.spec);
                    let name = PackageName::from_str(&named_spec.name).map_err(|err| {
                        DependenciesError::InvalidPackageName(named_spec.name.to_owned(), err)
                    })?;
                    Ok((name, spec))
                })
                .collect()
        }

        fn convert_binary_spec(
            specs: &[NamedSpecV1<BinaryPackageSpecV1>],
        ) -> Result<DependencyMap<PackageName, BinarySpec>, DependenciesError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let spec = conversion::from_binary_spec_v1(named_spec.spec);
                    let name = PackageName::from_str(&named_spec.name).map_err(|err| {
                        DependenciesError::InvalidPackageName(named_spec.name.to_owned(), err)
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
