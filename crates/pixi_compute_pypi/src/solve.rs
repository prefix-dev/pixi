//! Resolves (solves) the PyPI side of an environment through the compute
//! engine.
//!
//! Callers construct a [`SolvePypiEnvironmentSpec`] from their own
//! manifest/lock-file types and call
//! [`SolvePypiEnvironmentExt::solve_pypi_environment`]. The conda packages
//! of the environment must already be solved; conda-installed python
//! packages override their PyPI counterparts during resolution.

use indexmap::IndexMap;
use ordermap::OrderSet;
use pixi_compute_engine::ComputeCtx;
use pixi_compute_sources::RootDir;
use pixi_install_pypi::{
    LockedPypiRecord, UnresolvedPypiRecord, derive_link_mode,
    resolve::{CondaPrefixProvider, resolve_pypi},
};
use pixi_manifest::{
    PixiPlatform, SolveStrategy,
    pypi::{ResolvedPypiExcludeNewer, pypi_options::PypiOptions},
};
use pixi_pypi_spec::PixiPypiSpec;
use pixi_record::PixiRecord;
use pixi_utils::link_options::AllowLinkOptions;
use pixi_uv_conversions::to_exclude_newer;
use pixi_uv_reporter::UvReporterOptions;

use crate::data::HasUvResolutionContext;
use crate::reporter::HasSolvePypiReporter;

/// A specification for resolving the PyPI packages of an environment.
pub struct SolvePypiEnvironmentSpec {
    /// The name of the environment, only used for progress reporting.
    pub name: String,

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

    /// When set, fail instead of installing a conda prefix when a source
    /// distribution must be built (e.g. `--no-install`).
    pub disallow_install_conda_prefix: bool,

    /// Exclude distributions uploaded after the given cutoffs.
    pub exclude_newer: ResolvedPypiExcludeNewer,

    /// The resolution strategy (highest, lowest, lowest-direct).
    pub solve_strategy: SolveStrategy,
}

/// Resolve the PyPI packages of an environment through the compute engine.
///
/// The shared uv context and the workspace root come from the engine's data
/// store; progress is reported through the registered
/// [`SolvePypiReporter`](crate::SolvePypiReporter), if any.
pub trait SolvePypiEnvironmentExt {
    /// Resolve the PyPI packages of an environment.
    ///
    /// Returns the locked PyPI records for the requested dependencies,
    /// resolved against the conda records in the spec.
    ///
    /// `prefix_provider` supplies a conda prefix (python interpreter plus
    /// activation environment) on demand; it is only invoked when a source
    /// distribution actually has to be built to obtain metadata.
    fn solve_pypi_environment(
        &mut self,
        spec: SolvePypiEnvironmentSpec,
        prefix_provider: &dyn CondaPrefixProvider,
    ) -> impl Future<Output = miette::Result<Vec<LockedPypiRecord>>>;
}

impl SolvePypiEnvironmentExt for ComputeCtx {
    async fn solve_pypi_environment(
        &mut self,
        spec: SolvePypiEnvironmentSpec,
        prefix_provider: &dyn CondaPrefixProvider,
    ) -> miette::Result<Vec<LockedPypiRecord>> {
        let data = self.global_data();
        let uv_context = data.uv_resolution_context()?.clone();
        let root_dir = data.get::<RootDir>().to_path_buf();
        let link_options = data
            .try_get::<AllowLinkOptions>()
            .copied()
            .unwrap_or_default();
        let link_mode = derive_link_mode(
            link_options.allow_symbolic_links,
            link_options.allow_hard_links,
            link_options.allow_ref_links,
        );

        // Reporter lifecycle for this solve.
        let reporter = data.solve_pypi_reporter().cloned();
        let reporter_id = reporter
            .as_deref()
            .map(|r| r.on_queued(&spec.name, spec.platform.name().as_str()));
        if let (Some(r), Some(id)) = (reporter.as_deref(), reporter_id) {
            r.on_started(id);
        }
        let uv_reporter = match (reporter.as_deref(), reporter_id) {
            (Some(r), Some(id)) => r.create_uv_reporter(id, UvReporterOptions::new()),
            _ => None,
        };

        let result = resolve_pypi(
            uv_context,
            &spec.pypi_options,
            spec.dependencies,
            &spec.pixi_records,
            &spec.locked_pypi_records,
            &spec.platform,
            uv_reporter,
            root_dir.as_std_path(),
            prefix_provider,
            spec.disallow_install_conda_prefix,
            to_exclude_newer(&spec.exclude_newer),
            spec.solve_strategy,
            link_mode,
        )
        .await;

        if let (Some(r), Some(id)) = (reporter.as_deref(), reporter_id) {
            r.on_finished(id);
        }

        result
    }
}
