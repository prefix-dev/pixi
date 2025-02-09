use indexmap::IndexMap;
use miette::IntoDiagnostic;
use pep508_rs::Requirement;
use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness};

use crate::Workspace;
use pixi_manifest::pypi::PyPiPackageName;

/// A trait to facilitate extraction of packages data from arguments
pub(crate) trait HasSpecs {
    /// returns packages passed as arguments to the command
    fn packages(&self) -> Vec<&str>;

    fn specs(&self) -> miette::Result<IndexMap<PackageName, MatchSpec>> {
        self.packages()
            .iter()
            .map(|package| {
                let spec =
                    MatchSpec::from_str(package, ParseStrictness::Lenient).into_diagnostic()?;
                let name = spec.name.clone().ok_or_else(|| {
                    miette::miette!("could not find package name in MatchSpec {}", spec)
                })?;
                Ok((name, spec))
            })
            .collect()
    }

    fn pypi_deps(
        &self,
        project: &Workspace,
    ) -> miette::Result<IndexMap<PyPiPackageName, Requirement>> {
        self.packages()
            .iter()
            .map(|package| {
                let dep = Requirement::parse(package, project.root()).into_diagnostic()?;
                let name = PyPiPackageName::from_normalized(dep.clone().name);
                Ok((name, dep))
            })
            .collect()
    }
}
