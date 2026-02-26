use std::sync::Arc;

use minijinja::Value;
use ordermap::OrderMap;
use pixi_build_types::{
    BinaryPackageSpec, PackageSpec, SourcePackageSpec, Target, TargetSelector, Targets,
    procedures::conda_build_v1::{
        CondaBuildV1Dependency, CondaBuildV1DependencySource, CondaBuildV1Prefix,
        CondaBuildV1RunExports,
    },
};
use rattler_build::render::resolved_dependencies::{
    DependencyInfo, FinalizedDependencies, FinalizedRunDependencies, ResolvedDependencies,
    RunExportDependency, SourceDependency,
};
use rattler_build_jinja::Variable;
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, PackageNameMatcher, package::RunExportsJson,
};
use recipe_stage0::{
    matchspec::{PackageDependency, SourceMatchSpec},
    recipe::{Conditional, ConditionalList, ConditionalRequirements, Item, ListOrItem},
    requirements::PackageSpecDependencies,
};
use serde::Deserialize;
use url::Url;

use crate::encoded_source_spec_url::EncodedSourceSpecUrl;

pub fn from_source_url_to_source_package(source_url: Url) -> Option<SourcePackageSpec> {
    match source_url.scheme() {
        "source" => Some(EncodedSourceSpecUrl::from(source_url).into()),
        _ => None,
    }
}

pub fn from_source_matchspec_into_package_spec(
    source_matchspec: SourceMatchSpec,
) -> miette::Result<SourcePackageSpec> {
    from_source_url_to_source_package(source_matchspec.location)
        .ok_or_else(|| miette::miette!("Only file, http/https and git are supported for now"))
}

#[derive(Debug, Clone)]
pub enum PlatformKind {
    Build,
    Host,
    Target,
}

impl std::fmt::Display for PlatformKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlatformKind::Build => write!(f, "build"),
            PlatformKind::Host => write!(f, "host"),
            PlatformKind::Target => write!(f, "target"),
        }
    }
}

pub fn convert_variant_from_pixi_build_types(variant: pixi_build_types::VariantValue) -> Variable {
    match variant {
        pixi_build_types::VariantValue::String(s) => Variable::from(s),
        pixi_build_types::VariantValue::Int(i) => Variable::from(i),
        pixi_build_types::VariantValue::Bool(b) => Variable::from(b),
    }
}

pub fn convert_variant_to_pixi_build_types(
    variant: Variable,
) -> Result<pixi_build_types::VariantValue, minijinja::Error> {
    let value = Value::from(variant);
    pixi_build_types::VariantValue::deserialize(value)
}

pub fn to_rattler_build_selector(selector: &TargetSelector, platform_kind: PlatformKind) -> String {
    match selector {
        TargetSelector::Platform(p) => format!("{platform_kind}_platform == '{p}'"),
        _ => selector.to_string(),
    }
}

pub fn from_targets_v1_to_conditional_requirements(targets: &Targets) -> ConditionalRequirements {
    let mut build_items = ConditionalList::new();
    let mut host_items = ConditionalList::new();
    let mut run_items = ConditionalList::new();
    let run_constraints_items = ConditionalList::new();

    // Add default target
    if let Some(default_target) = &targets.default_target {
        let package_requirements = target_to_package_spec(default_target);

        // source_target_requirements.default_target = source_requirements;

        build_items.extend(
            package_requirements
                .build
                .into_iter()
                .map(|spec| spec.1)
                .map(Item::from),
        );

        host_items.extend(
            package_requirements
                .host
                .into_iter()
                .map(|spec| spec.1)
                .map(Item::from),
        );

        run_items.extend(
            package_requirements
                .run
                .into_iter()
                .map(|spec| spec.1)
                .map(Item::from),
        );
    }

    // Add specific targets
    if let Some(specific_targets) = &targets.targets {
        for (selector, target) in specific_targets {
            let package_requirements = target_to_package_spec(target);

            // Add the binary requirements
            // Use build_platform for build dependencies since they run on the build machine,
            // and host_platform for host/run dependencies since they target the host machine.
            build_items.extend(
                package_requirements
                    .build
                    .into_iter()
                    .map(|spec| spec.1)
                    .map(|spec| {
                        Conditional {
                            condition: to_rattler_build_selector(selector, PlatformKind::Build),
                            then: ListOrItem(vec![spec]),
                            else_value: ListOrItem::default(),
                        }
                        .into()
                    }),
            );
            host_items.extend(
                package_requirements
                    .host
                    .into_iter()
                    .map(|spec| spec.1)
                    .map(|spec| {
                        Conditional {
                            condition: to_rattler_build_selector(selector, PlatformKind::Host),
                            then: ListOrItem(vec![spec]),
                            else_value: ListOrItem::default(),
                        }
                        .into()
                    }),
            );
            run_items.extend(
                package_requirements
                    .run
                    .into_iter()
                    .map(|spec| spec.1)
                    .map(|spec| {
                        Conditional {
                            condition: to_rattler_build_selector(selector, PlatformKind::Host),
                            then: ListOrItem(vec![spec]),
                            else_value: ListOrItem::default(),
                        }
                        .into()
                    }),
            );
        }
    }

    ConditionalRequirements {
        build: build_items,
        host: host_items,
        run: run_items,
        run_constraints: run_constraints_items,
    }
}

pub(crate) fn source_package_spec_to_package_dependency(
    name: PackageName,
    source_spec: SourcePackageSpec,
) -> miette::Result<SourceMatchSpec> {
    let spec = MatchSpec {
        name: Some(PackageNameMatcher::Exact(name)),
        ..Default::default()
    };

    Ok(SourceMatchSpec {
        spec,
        location: EncodedSourceSpecUrl::from(source_spec).into(),
    })
}

fn binary_package_spec_to_package_dependency(
    name: PackageName,
    binary_spec: BinaryPackageSpec,
) -> PackageDependency {
    let BinaryPackageSpec {
        version,
        build,
        build_number,
        file_name,
        channel,
        subdir,
        md5,
        sha256,
        url,
        license,
    } = binary_spec;

    // If the version is "*", we treat it as None
    // so later rattler-build can detect the PackageDependency as a variant.
    let version = version.filter(|v| v != &rattler_conda_types::VersionSpec::Any);

    PackageDependency::Binary(MatchSpec {
        name: Some(PackageNameMatcher::Exact(name)),
        version,
        build,
        build_number,
        file_name,
        extras: None,
        channel: channel.map(Channel::from_url).map(Arc::new),
        subdir,
        namespace: None,
        md5,
        sha256,
        url,
        license,
        condition: None,
        track_features: None,
    })
}

fn package_spec_to_package_dependency(
    name: PackageName,
    spec: PackageSpec,
) -> miette::Result<PackageDependency> {
    match spec {
        PackageSpec::Binary(binary_spec) => {
            Ok(binary_package_spec_to_package_dependency(name, binary_spec))
        }
        PackageSpec::Source(source_spec) => Ok(PackageDependency::Source(
            source_package_spec_to_package_dependency(name, source_spec)?,
        )),
        PackageSpec::PinCompatible(_) => {
            miette::bail!("PinCompatible package specs are not yet supported in this context")
        }
    }
}

pub(crate) fn package_specs_to_package_dependency(
    specs: OrderMap<String, PackageSpec>,
) -> miette::Result<Vec<PackageDependency>> {
    specs
        .into_iter()
        .map(|(name, spec)| {
            package_spec_to_package_dependency(PackageName::new_unchecked(name), spec)
        })
        .collect()
}

// TODO: Should it be a From implementation?
pub fn target_to_package_spec(target: &Target) -> PackageSpecDependencies<PackageDependency> {
    let build_reqs = target
        .clone()
        .build_dependencies
        .map(|deps| package_specs_to_package_dependency(deps).unwrap())
        .unwrap_or_default();

    let host_reqs = target
        .clone()
        .host_dependencies
        .map(|deps| package_specs_to_package_dependency(deps).unwrap())
        .unwrap_or_default();

    let run_reqs = target
        .clone()
        .run_dependencies
        .map(|deps| package_specs_to_package_dependency(deps).unwrap())
        .unwrap_or_default();

    let mut bin_reqs = PackageSpecDependencies::default();

    for spec in build_reqs.iter() {
        bin_reqs.build.insert(spec.package_name(), spec.clone());
    }

    for spec in host_reqs.iter() {
        bin_reqs.host.insert(spec.package_name(), spec.clone());
    }

    for spec in run_reqs.iter() {
        bin_reqs.run.insert(spec.package_name(), spec.clone());
    }

    bin_reqs
}

pub(crate) fn from_build_v1_dependency_to_dependency_info(
    spec: CondaBuildV1Dependency,
) -> DependencyInfo {
    match spec.source {
        Some(CondaBuildV1DependencySource::RunExport(run_export)) => {
            DependencyInfo::RunExport(RunExportDependency {
                spec: spec.spec,
                from: run_export.from,
                source_package: run_export.package_name.as_normalized().to_string(),
            })
        }
        None => DependencyInfo::Source(SourceDependency { spec: spec.spec }),
    }
}

pub(crate) fn from_build_v1_run_exports_to_run_exports(
    run_exports: CondaBuildV1RunExports,
) -> RunExportsJson {
    RunExportsJson {
        weak: run_exports
            .weak
            .into_iter()
            .map(|dep| dep.spec.to_string())
            .collect(),
        strong: run_exports
            .strong
            .into_iter()
            .map(|dep| dep.spec.to_string())
            .collect(),
        noarch: run_exports
            .noarch
            .into_iter()
            .map(|dep| dep.spec.to_string())
            .collect(),
        strong_constrains: run_exports
            .strong_constrains
            .into_iter()
            .map(|dep| dep.spec.to_string())
            .collect(),
        weak_constrains: run_exports
            .weak_constrains
            .into_iter()
            .map(|dep| dep.spec.to_string())
            .collect(),
    }
}

pub fn from_build_v1_args_to_finalized_dependencies(
    build_prefix: Option<CondaBuildV1Prefix>,
    host_prefix: Option<CondaBuildV1Prefix>,
    run_dependencies: Option<Vec<CondaBuildV1Dependency>>,
    run_constraints: Option<Vec<CondaBuildV1Dependency>>,
    run_exports: Option<CondaBuildV1RunExports>,
) -> FinalizedDependencies {
    FinalizedDependencies {
        build: build_prefix.map(|prefix| ResolvedDependencies {
            specs: prefix
                .dependencies
                .into_iter()
                .map(from_build_v1_dependency_to_dependency_info)
                .collect(),
            resolved: prefix
                .packages
                .into_iter()
                .map(|pkg| pkg.repodata_record)
                .collect(),
        }),
        host: host_prefix.map(|prefix| ResolvedDependencies {
            specs: prefix
                .dependencies
                .into_iter()
                .map(from_build_v1_dependency_to_dependency_info)
                .collect(),
            resolved: prefix
                .packages
                .into_iter()
                .map(|pkg| pkg.repodata_record)
                .collect(),
        }),
        run: FinalizedRunDependencies {
            depends: run_dependencies
                .unwrap_or_default()
                .into_iter()
                .map(from_build_v1_dependency_to_dependency_info)
                .collect(),
            constraints: run_constraints
                .unwrap_or_default()
                .into_iter()
                .map(from_build_v1_dependency_to_dependency_info)
                .collect(),
            run_exports: run_exports
                .map(from_build_v1_run_exports_to_run_exports)
                .unwrap_or_default(),
        },
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_binary_package_conversion() {
        let name = PackageName::new_unchecked("foobar");
        let spec = BinaryPackageSpec {
            version: Some("3.12.*".parse().unwrap()),
            ..BinaryPackageSpec::default()
        };
        let match_spec = binary_package_spec_to_package_dependency(name, spec);
        assert_eq!(match_spec.to_string(), "foobar 3.12.*");
    }

    #[test]
    fn test_binary_package_conversion_any_is_treated_as_none() {
        let name = PackageName::new_unchecked("python");
        let spec = BinaryPackageSpec {
            version: Some("*".parse().unwrap()),
            ..BinaryPackageSpec::default()
        };
        let match_spec = binary_package_spec_to_package_dependency(name, spec);
        assert_eq!(match_spec.to_string(), "python");
    }
}
