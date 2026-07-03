use std::{fmt::Display, hash::Hash, str::FromStr, sync::Arc};

use super::conversion;
use crate::build::pin_compatible::{
    PinCompatibilityMap, PinCompatibleError, resolve_pin_compatible,
};
use pixi_build_types as pbt;
use pixi_build_types::{
    ExtraGroupName, NamedSpec, PackageSpec,
    procedures::conda_outputs::{
        CondaOutputDependencies, CondaOutputIgnoreRunExports, CondaOutputRunExports,
    },
};
use pixi_record::PixiRecord;
use pixi_spec::{
    BinarySpec, DetailedSpec, MatchspecFields, PixiSpec, SourceAnchor, SourceLocationSpec,
    SourceSpec, UrlBinarySpec,
};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    InvalidPackageNameError, MatchSpec, NamedChannelOrUrl, NamelessMatchSpec, PackageName,
    ParseMatchSpecOptions, Platform, RepodataRevision, VersionSpec,
};
use rattler_repodata_gateway::{Gateway, RunExportExtractorError, RunExportsReporter};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, thiserror::Error)]
pub enum DependenciesError {
    #[error(transparent)]
    InvalidPackageName(Arc<InvalidPackageNameError>),

    #[error(transparent)]
    PinCompatibleError(#[from] PinCompatibleError),
}

impl From<InvalidPackageNameError> for DependenciesError {
    fn from(err: InvalidPackageNameError) -> Self {
        Self::InvalidPackageName(Arc::new(err))
    }
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

/// Convert a single protocol [`PackageSpec`] into a [`PixiSpec`], resolving
/// source specs against `source_anchor` and pin-compatible specs against
/// `compatibility_map`.
fn package_spec_to_pixi_spec(
    name: &PackageName,
    spec: &PackageSpec,
    source_anchor: Option<&SourceAnchor>,
    compatibility_map: &PinCompatibilityMap<'_>,
) -> Result<PixiSpec, DependenciesError> {
    match spec {
        pbt::PackageSpec::Binary(binary) => Ok(PixiSpec::from(conversion::from_binary_spec_v1(
            (**binary).clone(),
        ))),
        pbt::PackageSpec::Source(source) => {
            let spec = conversion::from_source_spec_v1(source.clone());
            Ok(PixiSpec::from(match source_anchor {
                Some(anchor) => spec.resolve(anchor),
                None => spec,
            }))
        }
        pbt::PackageSpec::PinCompatible(pin) => {
            Ok(resolve_pin_compatible(name, pin, compatibility_map)?)
        }
    }
}

/// Resolve the extra groups from a backend output into
/// per-group [`DependencyMap`]s, applying the same source/pin resolution as the
/// regular run dependencies. Resolving source specs against `source_anchor`
/// lets them be registered as source dependencies of the produced record, so
/// extras can pull in source packages just like `run-dependencies`.
pub fn convert_extra_dependencies(
    extra_dependencies: &BTreeMap<ExtraGroupName, Vec<NamedSpec<PackageSpec>>>,
    source_anchor: Option<SourceAnchor>,
    compatibility_map: &PinCompatibilityMap<'_>,
) -> Result<BTreeMap<ExtraGroupName, DependencyMap<PackageName, PixiSpec>>, DependenciesError> {
    let mut groups = BTreeMap::new();
    for (group, specs) in extra_dependencies {
        let mut deps = DependencyMap::default();
        for named in specs {
            let name = PackageName::from_str(named.name.as_str())?;
            let spec = package_spec_to_pixi_spec(
                &name,
                &named.spec,
                source_anchor.as_ref(),
                compatibility_map,
            )?;
            deps.insert(name, spec);
        }
        groups.insert(group.clone(), deps);
    }
    Ok(groups)
}

impl Dependencies {
    pub fn new<'a>(
        output: &CondaOutputDependencies,
        source_anchor: Option<SourceAnchor>,
        compatibility_map: &PinCompatibilityMap<'a>,
    ) -> Result<Self, DependenciesError> {
        let mut dependencies = DependencyMap::default();
        let mut constraints = DependencyMap::default();

        for depend in &output.depends {
            let name = rattler_conda_types::PackageName::from_str(depend.name.as_str())?;
            let spec = package_spec_to_pixi_spec(
                &name,
                &depend.spec,
                source_anchor.as_ref(),
                compatibility_map,
            )?;
            dependencies.insert(name, spec.into());
        }

        for constraint in &output.constraints {
            let name = rattler_conda_types::PackageName::from_str(constraint.name.as_str())?;

            // Match on ConstraintSpec enum
            match &constraint.spec {
                pbt::ConstraintSpec::Binary(binary) => {
                    constraints
                        .insert(name, conversion::from_binary_spec_v1(binary.clone()).into());
                }
            }
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

        // Map every source-built package in the solved environment to its
        // pinned source location. A run-export naming one of these packages
        // must stay a source dependency on the assembled record; converted to
        // a plain binary matchspec, the consuming environment would look for
        // a channel package that doesn't exist.
        let source_locations: BTreeMap<PackageName, SourceLocationSpec> = records
            .iter()
            .filter_map(|record| match record {
                PixiRecord::Source(source) => Some((
                    source.package_record().name.clone(),
                    SourceLocationSpec::from(source.manifest_source().clone()),
                )),
                PixiRecord::Binary(_) => None,
            })
            .collect();

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
            .flat_map(|r| match r {
                PixiRecord::Binary(repo_data_record) => Some(Arc::make_mut(repo_data_record)),
                PixiRecord::Source(_) => None,
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
                noarch: filter_match_specs_with_sources(
                    &run_exports.noarch,
                    ignore,
                    &source_locations,
                ),
                strong: filter_match_specs_with_sources(
                    &run_exports.strong,
                    ignore,
                    &source_locations,
                ),
                weak: filter_match_specs_with_sources(&run_exports.weak, ignore, &source_locations),
                strong_constrains: filter_match_specs(&run_exports.strong_constrains, ignore),
                weak_constrains: filter_match_specs(&run_exports.weak_constrains, ignore),
            };

            combined_run_exports.push((record.name().clone(), filtered_run_exports));
        }

        Ok(combined_run_exports)
    }
}

pub fn filter_match_specs<T: From<BinarySpec> + Clone + Hash + Eq + PartialEq>(
    specs: &[String],
    ignore: &CondaOutputIgnoreRunExports,
) -> DependencyMap<PackageName, T> {
    specs
        .iter()
        .filter_map(move |spec| {
            let (name, spec) = parse_run_export_spec(spec, ignore)?;
            Some((name, binary_spec_from_nameless(spec).into()))
        })
        .collect()
}

/// Like [`filter_match_specs`], but run-export specs whose package name maps
/// to an entry in `source_locations` become source specs carrying that
/// location (the matchspec selectors are preserved). Specs with an explicit
/// URL stay binary: they pin a concrete artifact.
fn filter_match_specs_with_sources(
    specs: &[String],
    ignore: &CondaOutputIgnoreRunExports,
    source_locations: &BTreeMap<PackageName, SourceLocationSpec>,
) -> DependencyMap<PackageName, PixiSpec> {
    specs
        .iter()
        .filter_map(move |spec| {
            let (name, spec) = parse_run_export_spec(spec, ignore)?;
            let pixi_spec = match source_locations.get(&name) {
                Some(location) if spec.url.is_none() => SourceSpec {
                    location: location.clone(),
                    matchspec: MatchspecFields::from_nameless_match_spec(&spec),
                }
                .into(),
                _ => PixiSpec::from(binary_spec_from_nameless(spec)),
            };
            Some((name, pixi_spec))
        })
        .collect()
}

/// Parse a run-export matchspec string, dropping unparsable specs, non-exact
/// names, and names listed in `ignore.by_name`.
fn parse_run_export_spec(
    spec: &str,
    ignore: &CondaOutputIgnoreRunExports,
) -> Option<(PackageName, NamelessMatchSpec)> {
    let (name_matcher, spec) = MatchSpec::from_str(
        spec,
        ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
    )
    .ok()?
    .into_nameless();
    let name = name_matcher.as_exact().cloned()?;
    if ignore.by_name.contains(&name) {
        return None;
    }
    Some((name, spec))
}

fn binary_spec_from_nameless(spec: NamelessMatchSpec) -> BinarySpec {
    match spec {
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
            flags: None,
            channel: None,
            subdir: None,
            namespace: None,
            md5: None,
            sha256: None,
            url: _,
            license: None,
            license_family: None,
            condition: None,
            track_features: None,
        } => BinarySpec::Version(version.unwrap_or(VersionSpec::Any)),
        NamelessMatchSpec {
            version,
            build,
            build_number,
            file_name,
            extras,
            flags,
            channel,
            subdir,
            md5,
            sha256,
            license,
            license_family,
            condition,
            track_features,

            // Caught in the above case
            url: _,

            // Explicitly ignored
            namespace: _,
        } => BinarySpec::DetailedVersion(Box::new(DetailedSpec {
            version,
            build,
            build_number,
            file_name,
            extras,
            flags,
            channel: channel.map(|c| NamedChannelOrUrl::Url(c.base_url.clone().into())),
            subdir,
            md5,
            sha256,
            license,
            license_family,
            condition,
            track_features,
        })),
    }
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
    pub fn try_from_protocol<'a>(
        output: &CondaOutputRunExports,
        compatibility_map: &PinCompatibilityMap<'a>,
    ) -> Result<Self, DependenciesError> {
        fn convert_package_spec<'a>(
            specs: &[NamedSpec<PackageSpec>],
            compatibility_map: &PinCompatibilityMap<'a>,
        ) -> Result<DependencyMap<PackageName, PixiSpec>, DependenciesError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let name = PackageName::from_str(named_spec.name.as_str())?;

                    let spec = match named_spec.spec {
                        pbt::PackageSpec::Binary(binary) => {
                            conversion::from_binary_spec_v1(*binary).into()
                        }
                        pbt::PackageSpec::Source(source) => {
                            conversion::from_source_spec_v1(source).into()
                        }
                        pbt::PackageSpec::PinCompatible(pin) => {
                            resolve_pin_compatible(&name, &pin, compatibility_map)?
                        }
                    };

                    Ok((name, spec))
                })
                .collect()
        }

        fn convert_constraint_spec(
            specs: &[NamedSpec<pbt::ConstraintSpec>],
        ) -> Result<DependencyMap<PackageName, BinarySpec>, DependenciesError> {
            specs
                .iter()
                .cloned()
                .map(|named_spec| {
                    let name = PackageName::from_str(named_spec.name.as_str())?;

                    // Match on ConstraintSpec enum
                    let spec = match named_spec.spec {
                        pbt::ConstraintSpec::Binary(binary) => {
                            conversion::from_binary_spec_v1(binary)
                        }
                    };

                    Ok((name, spec))
                })
                .collect()
        }

        Ok(PixiRunExports {
            weak: convert_package_spec(&output.weak, compatibility_map)?,
            strong: convert_package_spec(&output.strong, compatibility_map)?,
            noarch: convert_package_spec(&output.noarch, compatibility_map)?,
            weak_constrains: convert_constraint_spec(&output.weak_constrains)?,
            strong_constrains: convert_constraint_spec(&output.strong_constrains)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use pixi_build_types::{ExtraGroupName, NamedSpec, PackageSpec, PathSpec, SourcePackageName};
    use rattler_conda_types::PackageName;

    use super::convert_extra_dependencies;

    /// A source dependency inside an extra group must be resolved as a source
    /// spec rather than stringified into a meaningless binary match spec, so
    /// that source packages can be pulled in through extras. Regression guard
    /// for extras dropping source dependencies.
    #[test]
    fn source_dependency_in_extra_group_is_preserved_as_source() {
        let dep_name = SourcePackageName::from(PackageName::new_unchecked("mydep"));
        let source_spec = PackageSpec::Source(
            PathSpec {
                path: "./mydep".to_string(),
            }
            .into(),
        );

        let mut extras = BTreeMap::new();
        extras.insert(
            ExtraGroupName::new("test").unwrap(),
            vec![NamedSpec {
                name: dep_name,
                spec: source_spec,
            }],
        );

        let resolved = convert_extra_dependencies(&extras, None, &HashMap::new()).unwrap();
        let group = resolved
            .get(&ExtraGroupName::new("test").unwrap())
            .expect("test group is present");
        let (name, spec) = group
            .iter_specs()
            .next()
            .expect("the group has one dependency");
        assert_eq!(name.as_normalized(), "mydep");
        assert!(
            spec.is_source(),
            "a source dependency in an extra group must stay a source spec, got {spec:?}"
        );
    }
}
