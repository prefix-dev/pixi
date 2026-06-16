//! Solves (resolves) the PyPI side of an environment through the
//! [`CommandDispatcher`].
//!
//! This mirrors [`crate::install_pypi`] for resolution: callers construct a
//! [`SolvePypiEnvironmentSpec`] from their own manifest/lock-file types and
//! hand it to [`CommandDispatcher::solve_pypi_environment`], which drives the
//! uv-based resolver in `pixi_install_pypi::resolve`. The conda packages of
//! the environment must already be solved; conda-installed python packages
//! override their PyPI counterparts during resolution.

use std::{path::PathBuf, sync::Arc};

use indexmap::IndexMap;
use indicatif::ProgressBar;
use ordermap::OrderSet;
use pixi_install_pypi::{
    LockedPypiRecord, UnresolvedPypiRecord, derive_link_mode,
    resolve::{CondaPrefixProvider, LazyBuildDispatchDependencies, resolve_pypi},
};
use pixi_manifest::{
    PixiPlatform, SolveStrategy,
    pypi::{ResolvedPypiExcludeNewer, pypi_options::PypiOptions},
};
use pixi_pypi_spec::PixiPypiSpec;
use pixi_record::PixiRecord;
use pixi_uv_context::UvResolutionContext;
use pixi_uv_conversions::to_exclude_newer;

use crate::CommandDispatcher;

/// A specification for resolving the PyPI packages of an environment.
pub struct SolvePypiEnvironmentSpec {
    /// The requested PyPI dependencies.
    pub dependencies: IndexMap<uv_normalize::PackageName, OrderSet<PixiPypiSpec>>,

    /// The PyPI options (indexes, no-build, prerelease mode, ...) that apply
    /// to the environment.
    pub pypi_options: PypiOptions,

    /// The solved conda records of the environment for the target platform.
    /// Used to locate the python interpreter, derive wheel tags, and detect
    /// PyPI packages that are already installed by conda.
    pub pixi_records: Vec<PixiRecord>,

    /// Previously locked PyPI packages. Used as resolution preferences to
    /// minimize lock-file churn.
    pub locked_pypi_records: Vec<UnresolvedPypiRecord>,

    /// The platform to resolve for.
    pub platform: PixiPlatform,

    /// The directory against which relative paths (e.g. local wheels or
    /// editable installs) are resolved. For workspaces this is the directory
    /// that holds the lock file.
    pub project_root: PathBuf,

    /// When set, fail instead of installing a conda prefix when a source
    /// distribution must be built (e.g. `--no-install`).
    pub disallow_install_conda_prefix: bool,

    /// Exclude distributions uploaded after the given cutoffs.
    pub exclude_newer: ResolvedPypiExcludeNewer,

    /// The resolution strategy (highest, lowest, lowest-direct).
    pub solve_strategy: SolveStrategy,

    /// Cache of lazily initialized build-dispatch resources (interpreter,
    /// python environment, ...). Reusing the same cache across repeated
    /// solves of the same environment avoids re-querying the interpreter.
    pub build_dispatch_cache: Arc<LazyBuildDispatchDependencies>,

    /// The shared uv context (cache, concurrency, http settings) to use.
    pub uv_context: UvResolutionContext,

    /// Progress bar to report resolution progress on. A hidden bar is used
    /// when not provided.
    pub progress_bar: Option<ProgressBar>,
}

impl CommandDispatcher {
    /// Resolve the PyPI packages of an environment.
    ///
    /// Returns the locked PyPI records for the requested dependencies,
    /// resolved against the conda records in the spec.
    ///
    /// `prefix_provider` supplies a conda prefix (python interpreter plus
    /// activation environment) on demand; it is only invoked when a source
    /// distribution actually has to be built to obtain metadata.
    pub async fn solve_pypi_environment(
        &self,
        spec: SolvePypiEnvironmentSpec,
        prefix_provider: &dyn CondaPrefixProvider,
    ) -> miette::Result<Vec<LockedPypiRecord>> {
        let link_mode = derive_link_mode(
            self.allow_symbolic_links(),
            self.allow_hard_links(),
            self.allow_ref_links(),
        );
        let progress_bar = spec.progress_bar.unwrap_or_else(ProgressBar::hidden);

        resolve_pypi(
            spec.uv_context,
            &spec.pypi_options,
            spec.dependencies,
            &spec.pixi_records,
            &spec.locked_pypi_records,
            &spec.platform,
            &progress_bar,
            &spec.project_root,
            prefix_provider,
            spec.disallow_install_conda_prefix,
            to_exclude_newer(&spec.exclude_newer),
            spec.solve_strategy,
            &spec.build_dispatch_cache,
            link_mode,
        )
        .await
    }
}
