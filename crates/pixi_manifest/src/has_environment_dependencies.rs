use rattler_conda_types::Platform;

use crate::{CondaDependencies, FeaturesExt, HasManifestRef};

/// A trait that defines the dependencies of an environment.
pub trait HasEnvironmentDependencies<'source>:
    HasManifestRef<'source> + FeaturesExt<'source>
{
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
        self.combined_dependencies(platform)
    }
}
