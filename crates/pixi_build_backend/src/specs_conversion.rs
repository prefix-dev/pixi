use std::sync::Arc;

use minijinja::Value;
use ordermap::OrderMap;
use pixi_build_types::{
    BinaryPackageSpec, PackageSpec, SourcePackageName, SourcePackageSpec, Target, TargetSelector,
    Targets,
    procedures::conda_build_v1::{
        CondaBuildV1Dependency, CondaBuildV1DependencySource, CondaBuildV1Prefix,
        CondaBuildV1RunExports,
    },
};
use rattler_build_core::render::resolved_dependencies::{
    DependencyInfo, FinalizedDependencies, FinalizedRunDependencies, ResolvedDependencies,
    RunExportDependency, SourceDependency,
};
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage0::{
    Conditional, ConditionalList, Item, JinjaExpression, NestedItemList, Requirements,
    SerializableMatchSpec, Value as RecipeValue,
};

use crate::package_dependency::{PackageDependency, SourceMatchSpec};
use rattler_conda_types::{
    Channel, MatchSpec, PackageName, PackageNameMatcher, package::RunExportsJson,
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

/// Convert a `PackageDependency` to a `SerializableMatchSpec` for use in
/// rattler-build's `Requirements`.
fn package_dependency_to_matchspec(dep: PackageDependency) -> SerializableMatchSpec {
    dep.into()
}

/// Convert a `PackageDependency` into an `Item<SerializableMatchSpec>`.
fn package_dependency_to_item(dep: PackageDependency) -> Item<SerializableMatchSpec> {
    Item::Value(RecipeValue::new_concrete(
        package_dependency_to_matchspec(dep),
        None,
    ))
}

pub fn from_targets_v1_to_conditional_requirements(targets: &Targets) -> Requirements {
    let mut build_items = ConditionalList::default();
    let mut host_items = ConditionalList::default();
    let mut run_items = ConditionalList::default();
    let run_constraints_items = ConditionalList::default();

    // Add default target
    if let Some(default_target) = &targets.default_target {
        let package_requirements = PackageSpecDependencies::from(default_target);

        build_items.extend(
            package_requirements
                .build
                .into_iter()
                .map(|spec| spec.1)
                .map(package_dependency_to_item),
        );

        host_items.extend(
            package_requirements
                .host
                .into_iter()
                .map(|spec| spec.1)
                .map(package_dependency_to_item),
        );

        run_items.extend(
            package_requirements
                .run
                .into_iter()
                .map(|spec| spec.1)
                .map(package_dependency_to_item),
        );
    }

    // Add specific targets
    if let Some(specific_targets) = &targets.targets {
        for (selector, target) in specific_targets {
            let package_requirements = PackageSpecDependencies::from(target);
            let selector_str = to_rattler_build_selector(selector, PlatformKind::Host);

            // Helper to wrap a dep in a conditional
            let make_conditional = |dep: PackageDependency| -> Item<SerializableMatchSpec> {
                Item::Conditional(Conditional {
                    condition: JinjaExpression::new(selector_str.clone())
                        .expect("valid jinja expression"),
                    then: NestedItemList::single(package_dependency_to_item(dep)),
                    else_value: None,
                    condition_span: None,
                })
            };

            build_items.extend(
                package_requirements
                    .build
                    .into_iter()
                    .map(|spec| spec.1)
                    .map(make_conditional),
            );
            host_items.extend(
                package_requirements
                    .host
                    .into_iter()
                    .map(|spec| spec.1)
                    .map(make_conditional),
            );
            run_items.extend(
                package_requirements
                    .run
                    .into_iter()
                    .map(|spec| spec.1)
                    .map(make_conditional),
            );
        }
    }

    Requirements {
        build: build_items,
        host: host_items,
        run: run_items,
        run_constraints: run_constraints_items,
        ..Default::default()
    }
}

pub(crate) fn source_package_spec_to_package_dependency(
    name: PackageName,
    source_spec: SourcePackageSpec,
) -> miette::Result<SourceMatchSpec> {
    let spec = MatchSpec {
        name: PackageNameMatcher::Exact(name),
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
        condition,
    } = binary_spec;

    // If the version is "*", we treat it as None
    // so later rattler-build can detect the PackageDependency as a variant.
    let version = version.filter(|v| v != &rattler_conda_types::VersionSpec::Any);

    PackageDependency::Binary(MatchSpec {
        name: PackageNameMatcher::Exact(name),
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
        condition,
        track_features: None,
        flags: None,
        license_family: None,
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
    specs: OrderMap<SourcePackageName, PackageSpec>,
) -> miette::Result<Vec<PackageDependency>> {
    specs
        .into_iter()
        .map(|(name, spec)| {
            package_spec_to_package_dependency(PackageName::new_unchecked(name.as_str()), spec)
        })
        .collect()
}

/// A helper struct for organizing dependencies by type.
#[derive(Clone, Default)]
pub struct PackageSpecDependencies {
    pub build: indexmap::IndexMap<PackageName, PackageDependency>,
    pub host: indexmap::IndexMap<PackageName, PackageDependency>,
    pub run: indexmap::IndexMap<PackageName, PackageDependency>,
    pub run_constraints: indexmap::IndexMap<PackageName, PackageDependency>,
}

impl From<&Target> for PackageSpecDependencies {
    fn from(target: &Target) -> Self {
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
            if let Some(name) = spec.package_name() {
                bin_reqs.build.insert(name.clone(), spec.clone());
            }
        }

        for spec in host_reqs.iter() {
            if let Some(name) = spec.package_name() {
                bin_reqs.host.insert(name.clone(), spec.clone());
            }
        }

        for spec in run_reqs.iter() {
            if let Some(name) = spec.package_name() {
                bin_reqs.run.insert(name.clone(), spec.clone());
            }
        }

        bin_reqs
    }
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

    #[test]
    fn test_binary_package_conversion_preserves_condition() {
        use rattler_conda_types::{MatchSpecCondition, ParseMatchSpecOptions, RepodataRevision};

        let name = PackageName::new_unchecked("numpy");
        let condition = MatchSpecCondition::MatchSpec(Box::new(
            MatchSpec::from_str(
                "python >=3.10",
                ParseMatchSpecOptions::lenient().with_repodata_revision(RepodataRevision::V3),
            )
            .unwrap(),
        ));
        let spec = BinaryPackageSpec {
            version: Some("*".parse().unwrap()),
            condition: Some(condition.clone()),
            ..BinaryPackageSpec::default()
        };
        let match_spec = binary_package_spec_to_package_dependency(name, spec);
        let PackageDependency::Binary(match_spec) = match_spec else {
            panic!("expected binary dependency");
        };
        assert_eq!(match_spec.condition, Some(condition));
    }
}
