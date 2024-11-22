use rattler_conda_types::Platform;

use crate::{CondaDependencies, FeaturesExt, HasManifestRef, SpecType};

/// A trait that defines the dependencies of an environment.
pub trait HasEnvironmentDependencies<'source>:
    HasManifestRef<'source> + FeaturesExt<'source>
{
    /// Returns true if the run, host, and build dependencies of this
    /// environment should be combined or whether only the run dependencies
    /// should be used.
    fn should_combine_dependencies(&self) -> bool {
        // If the manifest has a build section defined we should not combine.
        if self.manifest().workspace.build_system.is_some() {
            return false;
        }
        true
    }

    /// Returns the dependencies that are requested by the user optionally for a
    /// specific platform.
    ///
    /// The dependencies returned from this function can be either the combined
    /// (run, host, build) dependencies or only the run dependencies. Which is
    /// returned is defined by the [`Self::should_combine_dependencies`] method.
    ///
    /// If the `platform` is `None` no platform specific dependencies are taken
    /// into consideration.
    fn environment_dependencies(&self, platform: Option<Platform>) -> CondaDependencies {
        if self.should_combine_dependencies() {
            self.combined_dependencies(platform)
        } else {
            self.dependencies(SpecType::Run, platform)
        }
    }
}
