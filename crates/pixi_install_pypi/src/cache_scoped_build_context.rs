//! A [`BuildContext`] that scopes uv's source-build cache to the conda
//! environment without leaking the discriminator to the PEP 517 backend.
//!
//! uv computes the built-wheel cache shard from [`BuildContext::config_settings`],
//! but the backend receives its config settings from the inner [`BuildDispatch`]'s
//! own field (read directly inside `BuildDispatch::setup_build`). By returning a
//! fingerprinted [`ConfigSettings`] here while the wrapped dispatch is built with
//! clean settings, source builds are cached per environment (issue #6226) yet no
//! synthetic option reaches strict backends like meson-python (issue #6271).

use std::path::Path;

use uv_configuration::{BuildKind, BuildOutput, NoSources};
use uv_dispatch::BuildDispatch;
use uv_distribution_filename::DistFilename;
use uv_distribution_types::{
    CachedDist, ConfigSettings, IsBuildBackendError, Requirement, SourceDist,
};
use uv_python::PythonEnvironment;
use uv_types::{BuildArena, BuildContext, BuildStack, ResolvedRequirements};

/// Wraps a [`BuildDispatch`], overriding only the config settings uv uses for the
/// build cache key. See the module docs.
pub(crate) struct CacheScopedBuildContext<'a> {
    inner: BuildDispatch<'a>,
    cache_config_settings: ConfigSettings,
}

impl<'a> CacheScopedBuildContext<'a> {
    pub(crate) fn new(inner: BuildDispatch<'a>, cache_config_settings: ConfigSettings) -> Self {
        Self {
            inner,
            cache_config_settings,
        }
    }
}

impl<'ctx> BuildContext for CacheScopedBuildContext<'ctx> {
    type SourceDistBuilder = <BuildDispatch<'ctx> as BuildContext>::SourceDistBuilder;

    /// The whole point of the wrapper: the cache shard is keyed by the
    /// fingerprinted settings, while the inner dispatch builds with its own
    /// (clean) ones.
    fn config_settings(&self) -> &ConfigSettings {
        &self.cache_config_settings
    }

    async fn interpreter(&self) -> &uv_python::Interpreter {
        self.inner.interpreter().await
    }

    fn cache(&self) -> &uv_cache::Cache {
        self.inner.cache()
    }

    fn git(&self) -> &uv_git::GitResolver {
        self.inner.git()
    }

    fn capabilities(&self) -> &uv_distribution_types::IndexCapabilities {
        self.inner.capabilities()
    }

    fn dependency_metadata(&self) -> &uv_distribution_types::DependencyMetadata {
        self.inner.dependency_metadata()
    }

    fn build_options(&self) -> &uv_configuration::BuildOptions {
        self.inner.build_options()
    }

    fn sources(&self) -> &NoSources {
        self.inner.sources()
    }

    fn locations(&self) -> &uv_distribution_types::IndexLocations {
        self.inner.locations()
    }

    async fn resolve<'a>(
        &'a self,
        requirements: &'a [Requirement],
        build_stack: &'a BuildStack,
    ) -> Result<ResolvedRequirements, impl IsBuildBackendError> {
        self.inner.resolve(requirements, build_stack).await
    }

    async fn install<'a>(
        &'a self,
        resolution: &'a ResolvedRequirements,
        venv: &'a PythonEnvironment,
        build_stack: &'a BuildStack,
    ) -> Result<Vec<CachedDist>, impl IsBuildBackendError> {
        self.inner.install(resolution, venv, build_stack).await
    }

    #[allow(clippy::too_many_arguments)]
    async fn setup_build<'a>(
        &'a self,
        source: &'a Path,
        subdirectory: Option<&'a Path>,
        install_path: &'a Path,
        version_id: Option<&'a str>,
        dist: Option<&'a SourceDist>,
        sources: &'a NoSources,
        build_kind: BuildKind,
        build_output: BuildOutput,
        build_stack: BuildStack,
    ) -> Result<Self::SourceDistBuilder, impl IsBuildBackendError> {
        self.inner
            .setup_build(
                source,
                subdirectory,
                install_path,
                version_id,
                dist,
                sources,
                build_kind,
                build_output,
                build_stack,
            )
            .await
    }

    async fn direct_build<'a>(
        &'a self,
        source: &'a Path,
        subdirectory: Option<&'a Path>,
        output_dir: &'a Path,
        sources: NoSources,
        build_kind: BuildKind,
        version_id: Option<&'a str>,
    ) -> Result<Option<DistFilename>, impl IsBuildBackendError> {
        self.inner
            .direct_build(
                source,
                subdirectory,
                output_dir,
                sources,
                build_kind,
                version_id,
            )
            .await
    }

    fn workspace_cache(&self) -> &uv_workspace::WorkspaceCache {
        self.inner.workspace_cache()
    }

    fn build_arena(&self) -> &BuildArena<Self::SourceDistBuilder> {
        self.inner.build_arena()
    }

    fn config_settings_package(&self) -> &uv_distribution_types::PackageConfigSettings {
        self.inner.config_settings_package()
    }

    fn extra_build_requires(&self) -> &uv_distribution_types::ExtraBuildRequires {
        self.inner.extra_build_requires()
    }

    fn build_isolation(&self) -> uv_types::BuildIsolation<'_> {
        self.inner.build_isolation()
    }

    fn extra_build_variables(&self) -> &uv_distribution_types::ExtraBuildVariables {
        self.inner.extra_build_variables()
    }
}
