use std::collections::{BTreeMap, HashMap};

use rattler_build::{
    NormalizedKey,
    recipe::{parser::Requirements, variable::Variable},
};
use serde::Serialize;

use crate::{
    PackageSpec, ProjectModel, Targets, dependencies::ExtractedDependencies, traits::Dependencies,
};

pub struct PackageRequirements<P: ProjectModel> {
    /// Requirements for rattler-build
    pub requirements: Requirements,

    /// The source requirements
    pub source: SourceRequirements<P>,
}

#[derive(Debug, Serialize)]
#[serde(bound(
    serialize = "<<P::Targets as Targets>::Spec as PackageSpec>::SourceSpec: Serialize"
))]
pub struct SourceRequirements<P: ProjectModel> {
    /// Source package specification for build dependencies
    pub build: HashMap<String, <<P::Targets as Targets>::Spec as PackageSpec>::SourceSpec>,

    /// Source package specification for host dependencies
    pub host: HashMap<String, <<P::Targets as Targets>::Spec as PackageSpec>::SourceSpec>,

    /// Source package specification for runtime dependencies
    pub run: HashMap<String, <<P::Targets as Targets>::Spec as PackageSpec>::SourceSpec>,
}

/// Return requirements for the given project model
pub fn requirements<P: ProjectModel>(
    dependencies: Dependencies<<P::Targets as Targets>::Spec>,
    variant: &BTreeMap<NormalizedKey, Variable>,
) -> miette::Result<PackageRequirements<P>> {
    let build = ExtractedDependencies::from_dependencies(dependencies.build, variant)?;
    let host = ExtractedDependencies::from_dependencies(dependencies.host, variant)?;
    let run = ExtractedDependencies::from_dependencies(dependencies.run, variant)?;

    Ok(PackageRequirements {
        requirements: Requirements {
            build: build.dependencies,
            host: host.dependencies,
            run: run.dependencies,
            ..Default::default()
        },
        source: SourceRequirements {
            build: build.sources,
            host: host.sources,
            run: run.sources,
        },
    })
}
