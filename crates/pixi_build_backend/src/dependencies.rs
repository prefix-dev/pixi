use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

use miette::{Context, Diagnostic, IntoDiagnostic};
use pixi_build_types as pbt;
use pixi_build_types::{BinaryPackageSpec, NamedSpec};
use rattler_build::{
    render::resolved_dependencies::{
        DependencyInfo, PinCompatibleDependency, PinSubpackageDependency, ResolveError,
        SourceDependency, VariantDependency,
    },
    types::PackageIdentifier,
};
use rattler_build_jinja::Variable;
use rattler_build_recipe::stage1::Dependency;
use rattler_build_types::NormalizedKey;
use rattler_build_types::{PinBound, PinError};
use rattler_conda_types::{
    MatchSpec, NamelessMatchSpec, PackageName, PackageNameMatcher, PackageRecord,
    ParseStrictness::Strict,
};
use thiserror::Error;

use crate::{
    specs_conversion::{convert_variant_from_pixi_build_types, from_source_url_to_source_package},
    traits::PackageSpec,
};

/// A helper struct to extract match specs from a manifest.
#[derive(Default)]
pub struct MatchspecExtractor<'a> {
    variant: Option<&'a BTreeMap<NormalizedKey, Variable>>,
}

pub struct ExtractedMatchSpecs<S: PackageSpec> {
    pub specs: Vec<MatchSpec>,
    pub sources: HashMap<String, S::SourceSpec>,
}

impl<'a> MatchspecExtractor<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the variant to use for the match specs.
    pub fn with_variant(self, variant: &'a BTreeMap<NormalizedKey, Variable>) -> Self {
        Self {
            variant: Some(variant),
        }
    }

    /// Extracts match specs from the given set of dependencies.
    pub fn extract<'b, S>(
        &self,
        dependencies: impl IntoIterator<Item = (&'b pbt::SourcePackageName, &'b S)>,
    ) -> miette::Result<ExtractedMatchSpecs<S>>
    where
        S: PackageSpec + 'b,
    {
        let mut specs = Vec::new();
        let mut source_specs = HashMap::new();
        for (name, spec) in dependencies.into_iter() {
            let name = PackageName::from_str(name.as_str()).into_diagnostic()?;
            // If we have a variant override, we should use that instead of the spec.
            if spec.can_be_used_as_variant()
                && let Some(variant_value) = self
                    .variant
                    .as_ref()
                    .and_then(|variant| variant.get(&NormalizedKey::from(&name)))
            {
                let spec = NamelessMatchSpec::from_str(
                    variant_value.as_ref().as_str().wrap_err_with(|| {
                        miette::miette!("Variant {variant_value} needs to be a string")
                    })?,
                    Strict,
                )
                .into_diagnostic()
                .context("failed to convert variant to matchspec")?;
                specs.push(MatchSpec::from_nameless(
                    spec,
                    Some(PackageNameMatcher::Exact(name)),
                ));
                continue;
            }

            // Match on supported packages
            let (match_spec, source_spec) = spec.to_match_spec(name.clone())?;

            specs.push(match_spec);
            if let Some(source_spec) = source_spec {
                source_specs.insert(name.as_normalized().to_owned(), source_spec);
            }
        }

        Ok(ExtractedMatchSpecs {
            specs,
            sources: source_specs,
        })
    }
}

pub struct ExtractedDependencies<T: PackageSpec> {
    pub dependencies: Vec<Dependency>,
    pub sources: HashMap<String, T::SourceSpec>,
}

impl<T: PackageSpec> ExtractedDependencies<T> {
    pub fn from_dependencies<'a>(
        dependencies: impl IntoIterator<Item = (&'a pbt::SourcePackageName, &'a T)>,
        variant: &BTreeMap<NormalizedKey, Variable>,
    ) -> miette::Result<Self>
    where
        T: 'a,
    {
        let extracted_specs = MatchspecExtractor::new()
            .with_variant(variant)
            .extract(dependencies)?;

        Ok(Self {
            dependencies: extracted_specs
                .specs
                .into_iter()
                .map(|s| Dependency::Spec(Box::new(s)))
                .collect(),
            sources: extracted_specs.sources,
        })
    }
}

/// Converts the input variant configuration passed from pixi to something that
/// rattler build can deal with.
pub fn convert_input_variant_configuration(
    variants: Option<BTreeMap<String, Vec<pixi_build_types::VariantValue>>>,
) -> Option<BTreeMap<NormalizedKey, Vec<Variable>>> {
    variants.map(|v| {
        v.into_iter()
            .map(|(k, v)| {
                (
                    k.into(),
                    v.into_iter()
                        .map(convert_variant_from_pixi_build_types)
                        .collect(),
                )
            })
            .collect()
    })
}

#[derive(Debug, Error, Diagnostic)]
pub enum ConvertDependencyError {
    #[error("only matchspecs with defined package names are supported")]
    MissingName,

    #[error("could not parse version spec for variant key {0}: {1}")]
    VariantSpecParseError(String, rattler_conda_types::ParseMatchSpecError),

    #[error("could not apply pin. The following subpackage is not available: {0:?}")]
    SubpackageNotFound(PackageName),

    #[error("could not apply pin: {0}")]
    PinApplyError(PinError),
}

fn convert_nameless_matchspec(spec: NamelessMatchSpec) -> pbt::BinaryPackageSpec {
    pbt::BinaryPackageSpec {
        version: spec.version,
        build: spec.build,
        build_number: spec.build_number,
        file_name: spec.file_name,
        channel: spec.channel.map(|c| c.base_url.clone().into()),
        subdir: spec.subdir,
        md5: spec.md5,
        sha256: spec.sha256,
        url: spec.url,
        license: spec.license,
    }
}

/// Checks if it is applicable to apply a variant on the specified match spec. A
/// variant can be applied if it has a name and no other fields set. Returns the
/// name of the variant that should be used.
fn can_apply_variant(spec: &MatchSpec) -> Option<&PackageName> {
    match &spec {
        MatchSpec {
            name: Some(name),
            version: None,
            build: None,
            build_number: None,
            file_name: None,
            extras: None,
            channel: None,
            subdir: None,
            namespace: None,
            md5: None,
            sha256: None,
            license: None,
            url: None,
            condition: None,
            track_features: None,
        } => name.as_exact(),
        _ => None,
    }
}

fn apply_variant_and_convert(
    spec: &MatchSpec,
    variant: &BTreeMap<NormalizedKey, Variable>,
) -> Result<Option<NamedSpec<BinaryPackageSpec>>, ConvertDependencyError> {
    let Some(name) = can_apply_variant(spec) else {
        return Ok(None);
    };
    let Some(version) = variant.get(&name.into()).map(Variable::to_string) else {
        return Ok(None);
    };

    // if the variant starts with an alphanumeric character,
    // we have to add a '=' to the version spec
    let mut spec = version.to_string();

    // check if all characters are alphanumeric or ., in that case add
    // a '=' to get "startswith" behavior
    if spec.chars().all(|c| c.is_alphanumeric() || c == '.') {
        spec = format!("={spec}");
    }

    let variant = name.as_normalized().to_string();
    let spec: NamelessMatchSpec = spec
        .parse()
        .map_err(|e| ConvertDependencyError::VariantSpecParseError(variant.clone(), e))?;

    Ok(Some(pbt::NamedSpec {
        name: name.as_source().to_owned(),
        spec: convert_nameless_matchspec(spec),
    }))
}

fn convert_dependency(
    dependency: Dependency,
    variant: &BTreeMap<NormalizedKey, Variable>,
    subpackages: &HashMap<PackageName, PackageIdentifier>,
    sources: &HashMap<String, pbt::SourcePackageSpec>,
) -> Result<pbt::NamedSpec<pbt::PackageSpec>, ConvertDependencyError> {
    let match_spec = match dependency {
        Dependency::Spec(spec) => {
            let spec = *spec;
            // Convert back to source spec if it is a source spec.
            if let Some(source_package) =
                spec.url.clone().and_then(from_source_url_to_source_package)
            {
                let Some(name_matcher) = spec.name else {
                    return Err(ConvertDependencyError::MissingName);
                };
                let Some(name) = name_matcher.as_exact() else {
                    return Err(ConvertDependencyError::MissingName);
                };
                return Ok(pbt::NamedSpec {
                    name: name.as_source().into(),
                    spec: pbt::PackageSpec::Source(source_package),
                });
            }

            // Apply a variant if it is applicable.
            if let Some(NamedSpec { name, spec }) = apply_variant_and_convert(&spec, variant)? {
                return Ok(pbt::NamedSpec {
                    name,
                    spec: pbt::PackageSpec::Binary(spec),
                });
            }
            spec
        }
        Dependency::PinSubpackage(pin) => {
            let name = &pin.pin_subpackage.name;
            let subpackage = subpackages
                .get(name)
                .ok_or(ConvertDependencyError::SubpackageNotFound(name.to_owned()))?;
            pin.pin_subpackage
                .apply(&subpackage.version, &subpackage.build_string)
                .map_err(ConvertDependencyError::PinApplyError)?
        }
        Dependency::PinCompatible(pin) => {
            let pin = &pin.pin_compatible;
            let name = &pin.name;
            let args = &pin.args;
            return Ok(pbt::NamedSpec {
                name: name.as_source().to_owned(),
                spec: pbt::PackageSpec::PinCompatible(pbt::PinCompatibleSpec {
                    lower_bound: args.lower_bound.clone().map(convert_pin_bound),
                    upper_bound: args.upper_bound.clone().map(convert_pin_bound),
                    exact: args.exact,
                    build: args.build.clone(),
                }),
            });
        }
    };

    let (Some(name_matcher), spec) = match_spec.into_nameless() else {
        return Err(ConvertDependencyError::MissingName);
    };
    let Some(name) = name_matcher.as_exact() else {
        return Err(ConvertDependencyError::MissingName);
    };

    if let Some(source_spec) = sources
        .get(name.as_source())
        .or_else(|| sources.get(name.as_normalized()))
    {
        let mut source_spec = source_spec.clone();
        // Merge in the spec details
        source_spec.version = spec.version.or(source_spec.version);
        source_spec.build = spec.build.or(source_spec.build);
        source_spec.build_number = spec.build_number.or(source_spec.build_number);
        source_spec.subdir = spec.subdir.or(source_spec.subdir);
        source_spec.license = spec.license.or(source_spec.license);

        Ok(pbt::NamedSpec {
            name: name.as_source().to_owned(),
            spec: pbt::PackageSpec::Source(source_spec),
        })
    } else {
        Ok(pbt::NamedSpec {
            name: name.as_source().to_owned(),
            spec: pbt::PackageSpec::Binary(convert_nameless_matchspec(spec)),
        })
    }
}

fn convert_pin_bound(pin_bound: PinBound) -> pbt::PinBound {
    match pin_bound {
        PinBound::Expression(expr) => {
            pbt::PinBound::Expression(pbt::PinExpression(expr.to_string()))
        }
        PinBound::Version(v) => pbt::PinBound::Version(v),
    }
}

fn convert_constraint_dependency(
    dependency: Dependency,
    variant: &BTreeMap<NormalizedKey, Variable>,
    subpackages: &HashMap<PackageName, PackageIdentifier>,
) -> Result<pbt::NamedSpec<pbt::ConstraintSpec>, ConvertDependencyError> {
    let match_spec = match dependency {
        Dependency::Spec(spec) => {
            let spec = *spec;
            // Apply a variant if it is applicable.
            if let Some(NamedSpec { spec, name }) = apply_variant_and_convert(&spec, variant)? {
                return Ok(NamedSpec {
                    spec: pbt::ConstraintSpec::Binary(spec),
                    name,
                });
            }
            spec
        }
        Dependency::PinSubpackage(pin) => {
            let name = &pin.pin_subpackage.name;
            let subpackage = subpackages
                .get(name)
                .ok_or(ConvertDependencyError::SubpackageNotFound(name.to_owned()))?;
            pin.pin_subpackage
                .apply(&subpackage.version, &subpackage.build_string)
                .map_err(ConvertDependencyError::PinApplyError)?
        }
        _ => todo!("Handle other dependency types"),
    };

    // Apply a variant if it is applicable.
    if let Some(NamedSpec { spec, name }) = apply_variant_and_convert(&match_spec, variant)? {
        return Ok(NamedSpec {
            spec: pbt::ConstraintSpec::Binary(spec),
            name,
        });
    }

    let (Some(name_matcher), spec) = match_spec.into_nameless() else {
        return Err(ConvertDependencyError::MissingName);
    };
    let Some(name) = name_matcher.as_exact() else {
        return Err(ConvertDependencyError::MissingName);
    };

    Ok(pbt::NamedSpec {
        name: name.as_source().to_owned(),
        spec: pbt::ConstraintSpec::Binary(convert_nameless_matchspec(spec)),
    })
}

pub fn convert_dependencies(
    dependencies: Vec<Dependency>,
    variant: &BTreeMap<NormalizedKey, Variable>,
    subpackages: &HashMap<PackageName, PackageIdentifier>,
    sources: &HashMap<String, pbt::SourcePackageSpec>,
) -> Result<Vec<pbt::NamedSpec<pbt::PackageSpec>>, ConvertDependencyError> {
    dependencies
        .into_iter()
        .map(|spec| convert_dependency(spec, variant, subpackages, sources))
        .collect()
}

pub fn convert_constraint_dependencies(
    dependencies: Vec<Dependency>,
    variant: &BTreeMap<NormalizedKey, Variable>,
    subpackages: &HashMap<PackageName, PackageIdentifier>,
) -> Result<Vec<pbt::NamedSpec<pbt::ConstraintSpec>>, ConvertDependencyError> {
    dependencies
        .into_iter()
        .map(|spec| convert_constraint_dependency(spec, variant, subpackages))
        .collect()
}

/// Apply a variant to a dependency list and resolve all pin_subpackage and
/// compiler dependencies
pub fn apply_variant(
    raw_specs: &[Dependency],
    variant: &BTreeMap<NormalizedKey, Variable>,
    subpackages: &HashMap<PackageName, PackageIdentifier>,
    compatibility_specs: &HashMap<PackageName, PackageRecord>,
    build_time: bool,
) -> Result<Vec<DependencyInfo>, ResolveError> {
    raw_specs
        .iter()
        .map(|s| {
            match s {
                Dependency::Spec(m) => {
                    let m = *m.clone();
                    if build_time
                        && m.version.is_none()
                        && m.build.is_none()
                        && let Some(name_matcher) = &m.name
                        && let Some(exact_name) = name_matcher.as_exact()
                        && let Some(version) = variant.get(&exact_name.into())
                    {
                        // if the variant starts with an alphanumeric character,
                        // we have to add a '=' to the version spec
                        let mut spec = version.to_string();

                        // check if all characters are alphanumeric or ., in that case add
                        // a '=' to get "startswith" behavior
                        if spec.chars().all(|c| c.is_alphanumeric() || c == '.') {
                            spec = format!("={spec}");
                        }

                        let variant = exact_name.as_normalized().to_string();
                        let spec: NamelessMatchSpec = spec
                            .parse()
                            .map_err(|e| ResolveError::VariantSpecParseError(variant.clone(), e))?;

                        let spec = MatchSpec::from_nameless(spec, Some(name_matcher.clone()));

                        return Ok(VariantDependency { spec, variant }.into());
                    }
                    Ok(SourceDependency { spec: m }.into())
                }
                Dependency::PinSubpackage(pin) => {
                    let name = &pin.pin_subpackage.name;
                    let subpackage = subpackages
                        .get(name)
                        .ok_or(ResolveError::PinSubpackageNotFound(name.to_owned()))?;
                    let pinned = pin
                        .pin_subpackage
                        .apply(&subpackage.version, &subpackage.build_string)?;
                    Ok(PinSubpackageDependency {
                        spec: pinned,
                        name: name.as_normalized().to_string(),
                        args: pin.pin_subpackage.args.clone(),
                    }
                    .into())
                }
                Dependency::PinCompatible(pin) => {
                    let name = &pin.pin_compatible.name;
                    let pin_package = compatibility_specs
                        .get(name)
                        .ok_or(ResolveError::PinCompatibleNotFound(name.to_owned()))?;

                    let pinned = pin
                        .pin_compatible
                        .apply(&pin_package.version, &pin_package.build)?;
                    Ok(PinCompatibleDependency {
                        spec: pinned,
                        name: name.as_normalized().to_string(),
                        args: pin.pin_compatible.args.clone(),
                    }
                    .into())
                }
            }
        })
        .collect()
}
