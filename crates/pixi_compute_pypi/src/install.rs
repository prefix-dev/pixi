//! Installs PyPI packages into a previously installed conda prefix through
//! the compute engine.
//!
//! Callers construct an [`InstallPypiEnvironmentSpec`] from their own
//! manifest/lock-file types and call
//! [`InstallPypiEnvironmentExt::install_pypi_environment`].

use std::{collections::HashSet, sync::Arc};

use pixi_compute_engine::ComputeCtx;
use pixi_compute_sources::RootDir;
use pixi_install_pypi::{
    InstallablePypiRecord, LazyEnvironmentVariables, PyPIBuildConfig, PyPIContextConfig,
    PyPIEnvironmentUpdater, PyPIUpdateConfig, derive_link_mode,
};
use pixi_manifest::{
    EnvironmentName, PixiPlatform,
    pypi::{
        ResolvedPypiExcludeNewer,
        pypi_options::{IndexStrategy, NoBinary, NoBuild, NoBuildIsolation},
    },
};
use pixi_python_status::PythonStatus;
use pixi_record::PixiRecord;
use pixi_utils::{link_options::AllowLinkOptions, prefix::Prefix};
use rattler_lock::PypiIndexes;

use crate::data::HasUvResolutionContext;
use crate::reporter::HasInstallPypiReporter;

/// A specification for installing PyPI packages into a conda prefix.
///
/// The conda packages of the environment must already be installed; this
/// spec only synchronizes the PyPI packages in the prefix's `site-packages`
/// with `pypi_records`.
pub struct InstallPypiEnvironmentSpec {
    /// The name of the environment, only used for progress reporting.
    pub name: EnvironmentName,

    /// The prefix in which the python interpreter lives and into which the
    /// packages are installed.
    pub prefix: Prefix,

    /// The platform for which the packages are installed.
    pub platform: PixiPlatform,

    /// The state of the python interpreter in the prefix, as reported by the
    /// conda install transaction. Installation is skipped when no
    /// interpreter is present, and outdated site-packages are removed when
    /// the interpreter changed.
    pub python_status: PythonStatus,

    /// The conda packages installed in the environment. Used to locate the
    /// python interpreter record and to derive wheel tags.
    pub pixi_records: Vec<PixiRecord>,

    /// The PyPI packages to install.
    pub pypi_records: Vec<InstallablePypiRecord>,

    /// The PyPI indexes the records were locked against.
    pub pypi_indexes: Option<PypiIndexes>,

    /// Packages that should be built without build isolation.
    pub no_build_isolation: NoBuildIsolation,

    /// Packages that must not be built from source.
    pub no_build: NoBuild,

    /// Packages that must be built from source.
    pub no_binary: NoBinary,

    /// The index strategy to use when fetching distributions.
    pub index_strategy: Option<IndexStrategy>,

    /// Exclude distributions uploaded after the given cutoffs.
    pub exclude_newer: ResolvedPypiExcludeNewer,

    /// Whether to skip the wheel filename check when installing wheels.
    pub skip_wheel_filename_check: Option<bool>,

    /// Package names that are never considered extraneous, i.e. they are not
    /// removed from the prefix even though they are missing from
    /// `pypi_records`.
    pub ignored_extraneous: HashSet<uv_normalize::PackageName>,

    /// Refresh the uv cache for all packages (`Some(true)`), no packages
    /// (`Some(false)`/`None`), or the specific packages listed in
    /// [`Self::cache_refresh_packages`]. Used to honor `--reinstall` flags.
    pub cache_refresh: Option<bool>,

    /// The packages whose uv cache entries are refreshed when
    /// [`Self::cache_refresh`] is `None`.
    pub cache_refresh_packages: Option<Vec<uv_normalize::PackageName>>,
}

/// Install the PyPI packages of an environment through the compute engine.
///
/// The shared uv context and the workspace root come from the engine's data
/// store; progress is reported through the registered
/// [`InstallPypiReporter`](crate::InstallPypiReporter), if any.
pub trait InstallPypiEnvironmentExt {
    /// Install PyPI packages into a previously installed conda prefix.
    ///
    /// This method takes the PyPI side of a previously solved environment and
    /// synchronizes the prefix's `site-packages` with it: missing packages
    /// are installed (downloading or building them as needed), outdated ones
    /// are reinstalled, and extraneous ones are removed.
    ///
    /// `env_variables` is resolved lazily, and only when a source
    /// distribution actually has to be built; workspace callers use it to
    /// expose the activated environment to PEP 517 backends. Pass `None`
    /// when no extra build environment is required.
    fn install_pypi_environment(
        &mut self,
        spec: InstallPypiEnvironmentSpec,
        env_variables: Option<&dyn LazyEnvironmentVariables>,
    ) -> impl Future<Output = miette::Result<()>>;
}

impl InstallPypiEnvironmentExt for ComputeCtx {
    async fn install_pypi_environment(
        &mut self,
        spec: InstallPypiEnvironmentSpec,
        env_variables: Option<&dyn LazyEnvironmentVariables>,
    ) -> miette::Result<()> {
        let data = self.global_data();
        let uv_context = data
            .uv_resolution_context()?
            .clone()
            .set_cache_refresh(spec.cache_refresh, spec.cache_refresh_packages.clone());
        let root_dir = data.get::<RootDir>().0.clone();
        let link_options = data
            .try_get::<AllowLinkOptions>()
            .copied()
            .unwrap_or_default();
        let link_mode = derive_link_mode(
            link_options.allow_symbolic_links,
            link_options.allow_hard_links,
            link_options.allow_ref_links,
        );

        // Reporter lifecycle for this install.
        let reporter = data.install_pypi_reporter().cloned();
        let reporter_id = reporter.as_deref().map(|r| r.on_queued(spec.name.as_str()));
        if let (Some(r), Some(id)) = (reporter.as_deref(), reporter_id) {
            r.on_started(id);
        }

        let update_config = PyPIUpdateConfig {
            environment_name: &spec.name,
            prefix: &spec.prefix,
            platform: &spec.platform,
            lock_file_dir: root_dir.as_std_path(),
        };

        let build_config = PyPIBuildConfig {
            no_build_isolation: &spec.no_build_isolation,
            no_build: &spec.no_build,
            no_binary: &spec.no_binary,
            index_strategy: spec.index_strategy.as_ref(),
            exclude_newer: &spec.exclude_newer,
            skip_wheel_filename_check: spec.skip_wheel_filename_check,
            link_mode: Some(link_mode),
        };

        let context_config = PyPIContextConfig {
            uv_context: &uv_context,
            pypi_indexes: spec.pypi_indexes.as_ref(),
            environment_variables_lazy: env_variables,
        };

        let mut updater = PyPIEnvironmentUpdater::new(update_config, build_config, context_config)
            .with_ignored_extraneous(spec.ignored_extraneous.clone());
        if let (Some(r), Some(id)) = (reporter.clone(), reporter_id) {
            updater = updater.with_uv_reporter_factory(Arc::new(move |options| {
                r.create_uv_reporter(id, options)
            }));
        }

        let result = updater
            .update(&spec.python_status, &spec.pixi_records, &spec.pypi_records)
            .await;

        if let (Some(r), Some(id)) = (reporter.as_deref(), reporter_id) {
            r.on_finished(id);
        }

        result
    }
}
