use pixi_pypi_spec::{PixiPypiSpec, PypiPackageName};
use pixi_spec::PixiSpec;
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::PackageName;

pub type PyPiDependencies = DependencyMap<PypiPackageName, PixiPypiSpec>;
pub type CondaDependencies = DependencyMap<PackageName, PixiSpec>;
