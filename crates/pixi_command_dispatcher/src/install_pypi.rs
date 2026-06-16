//! Installs PyPI packages into a previously installed conda prefix through
//! the [`CommandDispatcher`].
//!
//! This mirrors [`crate::install_pixi`] for the PyPI side of an environment:
//! callers (the workspace install pipeline, `pixi global`, ...) construct an
//! [`InstallPypiEnvironmentSpec`] from their own manifest/lock-file types and
//! hand it to [`CommandDispatcher::install_pypi_environment`], which drives
//! the uv-based installer in `pixi_install_pypi`.

use std::{collections::HashSet, path::PathBuf};

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
use pixi_utils::prefix::Prefix;
use pixi_uv_context::UvResolutionContext;
use rattler_lock::PypiIndexes;

use crate::CommandDispatcher;

/// A specification for installing PyPI packages into a conda prefix.
///
/// The conda packages of the environment must already be installed (see
/// [`crate::InstallPixiEnvironmentSpec`]); this spec only synchronizes the
/// PyPI packages in the prefix's `site-packages` with `pypi_records`.
pub struct InstallPypiEnvironmentSpec {
    /// The name of the environment, only used for progress reporting.
    pub name: EnvironmentName,

    /// The prefix in which the python interpreter lives and into which the
    /// packages are installed.
    pub prefix: Prefix,

    /// The platform for which the packages are installed.
    pub platform: PixiPlatform,

    /// The directory against which relative paths in the records (e.g. local
    /// wheels or editable installs) are resolved. For workspaces this is the
    /// directory that holds the lock file.
    pub lock_file_dir: PathBuf,

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

    /// The shared uv context (cache, concurrency, http settings) to use.
    pub uv_context: UvResolutionContext,
}

impl CommandDispatcher {
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
    pub async fn install_pypi_environment(
        &self,
        spec: InstallPypiEnvironmentSpec,
        env_variables: Option<&dyn LazyEnvironmentVariables>,
    ) -> miette::Result<()> {
        let update_config = PyPIUpdateConfig {
            environment_name: &spec.name,
            prefix: &spec.prefix,
            platform: &spec.platform,
            lock_file_dir: &spec.lock_file_dir,
        };

        let build_config = PyPIBuildConfig {
            no_build_isolation: &spec.no_build_isolation,
            no_build: &spec.no_build,
            no_binary: &spec.no_binary,
            index_strategy: spec.index_strategy.as_ref(),
            exclude_newer: &spec.exclude_newer,
            skip_wheel_filename_check: spec.skip_wheel_filename_check,
            link_mode: Some(derive_link_mode(
                self.allow_symbolic_links(),
                self.allow_hard_links(),
                self.allow_ref_links(),
            )),
        };

        let context_config = PyPIContextConfig {
            uv_context: &spec.uv_context,
            pypi_indexes: spec.pypi_indexes.as_ref(),
            environment_variables_lazy: env_variables,
        };

        PyPIEnvironmentUpdater::new(update_config, build_config, context_config)
            .with_ignored_extraneous(spec.ignored_extraneous)
            .update(&spec.python_status, &spec.pixi_records, &spec.pypi_records)
            .await
    }
}
