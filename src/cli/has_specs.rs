use indexmap::IndexMap;
use miette::IntoDiagnostic;
use pep508_rs::Requirement;
use rattler_conda_types::{MatchSpec, PackageName, ParseStrictness};

use crate::{project::manifest::python::PyPiPackageName, Project};

/// A trait to facilitate extraction of packages data from arguments
pub(crate) trait HasSpecs {
    /// returns packages passed as arguments to the command
    fn packages(&self) -> Vec<&str>;

    fn specs(&self) -> miette::Result<IndexMap<PackageName, MatchSpec>> {
        let mut map = IndexMap::with_capacity(self.packages().len());
        for package in self.packages() {
            let spec = MatchSpec::from_str(package, ParseStrictness::Strict).into_diagnostic()?;
            let name = spec.name.clone().ok_or_else(|| {
                miette::miette!("could not find package name in MatchSpec {}", spec)
            })?;
            map.insert(name, spec);
        }
        Ok(map)
    }

    fn pypi_deps(
        &self,
        project: &Project,
    ) -> miette::Result<IndexMap<PyPiPackageName, Requirement>> {
        let mut map = IndexMap::with_capacity(self.packages().len());
        for package in self.packages() {
            let dep = Requirement::parse(package, project.root()).into_diagnostic()?;
            let name = PyPiPackageName::from_normalized(dep.clone().name);
            map.insert(name, dep);
        }
        Ok(map)
    }
}
