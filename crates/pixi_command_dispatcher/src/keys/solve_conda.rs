//! Compute-engine Key that runs a conda solve. The spec is pure
//! hashable data (match specs, resolved source metadata, platform,
//! channels, exclude_newer, virtual packages). Binary repodata is
//! fetched inside the compute body to keep the Key's identity small.
//! The solve itself goes through the `SolveCondaExt::solve_conda`
//! ctx-extension trait (in `crate::solve_binary`) for concurrency
//! limiting and reporter wiring.

use std::{
    hash::{Hash, Hasher},
    mem,
    sync::Arc,
};

use derive_more::Display;
use itertools::Either;
use miette::Diagnostic;
use pixi_compute_engine::{ComputeCtx, Key};
use pixi_record::PixiRecord;
use pixi_spec::{BinarySpec, ResolvedExcludeNewer, SourceSpec, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{
    Channel, ChannelConfig, ChannelUrl, GenericVirtualPackage, MatchSpec, PackageName,
    PackageNameMatcher, ParseStrictness, Platform,
};
use rattler_repodata_gateway::GatewayError;
use rattler_solve::{ChannelPriority, SolveError, SolveStrategy};
use thiserror::Error;
use tracing::instrument;

use crate::{
    SolveCondaEnvironmentSpec, SourceMetadata, compute_data::HasGateway,
    solve_binary::SolveCondaExt, solve_conda::SolveCondaEnvironmentError,
};

/// Input to [`SolveCondaKey`]. All fields participate in the Key's
/// identity so two callers with equal specs share one compute.
#[derive(Debug, Clone)]
pub struct SolveCondaSpec {
    /// Source package requirements.
    pub source_specs: DependencyMap<PackageName, SourceSpec>,
    /// Binary package requirements.
    pub binary_specs: DependencyMap<PackageName, BinarySpec>,
    /// Constraints (binary-only).
    pub constraints: DependencyMap<PackageName, BinarySpec>,
    /// Dev source records (their dependencies are installed without
    /// building the packages themselves).
    pub dev_source_records: Vec<pixi_record::DevSourceRecord>,
    /// Already-assembled source metadata to feed to the solver.
    pub source_repodata: Vec<Arc<SourceMetadata>>,
    /// Already-installed records (hints to reduce solve drift).
    pub installed: Vec<PixiRecord>,
    /// Target platform.
    pub platform: Platform,
    /// Channels to search.
    pub channels: Vec<ChannelUrl>,
    /// Virtual packages to pretend are installed.
    pub virtual_packages: Vec<GenericVirtualPackage>,
    /// Solver strategy.
    pub strategy: SolveStrategy,
    /// Channel priority.
    pub channel_priority: ChannelPriority,
    /// Package exclusion cutoff.
    pub exclude_newer: Option<ResolvedExcludeNewer>,
}

impl Hash for SolveCondaSpec {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Destructure so adding a field forces a decision about its
        // hash contribution.
        //
        // TODO: once `SolveStrategy` + `ChannelPriority` implement
        // `Hash` (rattler PR https://github.com/conda/rattler/pull/2377),
        // this impl collapses into `derive(Hash, Eq, PartialEq)`.
        let Self {
            source_specs,
            binary_specs,
            constraints,
            dev_source_records,
            source_repodata,
            installed,
            platform,
            channels,
            virtual_packages,
            strategy,
            channel_priority,
            exclude_newer,
        } = self;
        source_specs.hash(state);
        binary_specs.hash(state);
        constraints.hash(state);
        dev_source_records.hash(state);
        // Hash Arc contents (not pointer identity): two equivalent
        // SourceMetadata Arcs from different sources should collide.
        source_repodata.len().hash(state);
        for sm in source_repodata {
            sm.hash(state);
        }
        installed.hash(state);
        platform.hash(state);
        channels.hash(state);
        virtual_packages.hash(state);
        mem::discriminant(strategy).hash(state);
        mem::discriminant(channel_priority).hash(state);
        exclude_newer.hash(state);
    }
}

impl PartialEq for SolveCondaSpec {
    // TODO: collapse to `derive(PartialEq, Eq)` once rattler PR
    // https://github.com/conda/rattler/pull/2377 lands.
    fn eq(&self, other: &Self) -> bool {
        self.source_specs == other.source_specs
            && self.binary_specs == other.binary_specs
            && self.constraints == other.constraints
            && self.dev_source_records == other.dev_source_records
            && self.source_repodata.len() == other.source_repodata.len()
            && self
                .source_repodata
                .iter()
                .zip(other.source_repodata.iter())
                .all(|(a, b)| **a == **b)
            && self.installed == other.installed
            && self.platform == other.platform
            && self.channels == other.channels
            && self.virtual_packages == other.virtual_packages
            && mem::discriminant(&self.strategy) == mem::discriminant(&other.strategy)
            && mem::discriminant(&self.channel_priority)
                == mem::discriminant(&other.channel_priority)
            && self.exclude_newer == other.exclude_newer
    }
}

impl Eq for SolveCondaSpec {}

/// Compute-engine Key wrapping a conda solve. Fetches binary repodata
/// from the gateway and runs the solver (semaphore-limited + reporter-
/// wired via the `SolveCondaExt` ctx-extension trait).
#[derive(Clone, Debug, Display)]
#[display("solve-conda[{}]", _0.platform)]
pub struct SolveCondaKey(pub Arc<SolveCondaSpec>);

impl SolveCondaKey {
    pub fn new(spec: SolveCondaSpec) -> Self {
        Self(Arc::new(spec))
    }
}

impl Hash for SolveCondaKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for SolveCondaKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Eq for SolveCondaKey {}

/// Clone-able error variant carried in [`SolveCondaKey::Value`]. The
/// underlying `SolveCondaEnvironmentError` contains error types that
/// are not themselves `Clone`, so we wrap each variant's payload in
/// an `Arc` here. Conversion helpers below bridge from the plain
/// error and into the broader [`crate::SolvePixiEnvironmentError`]
/// used by the orchestrator.
#[derive(Clone, Debug, Error, Diagnostic)]
pub enum SolveCondaKeyError {
    #[error("failed to solve the environment")]
    Solve(#[source] Arc<SolveError>),

    #[error(transparent)]
    SpecConversion(Arc<SpecConversionError>),

    #[error(transparent)]
    Gateway(Arc<GatewayError>),
}

impl From<SolveCondaEnvironmentError> for SolveCondaKeyError {
    fn from(err: SolveCondaEnvironmentError) -> Self {
        match err {
            SolveCondaEnvironmentError::SolveError(e) => SolveCondaKeyError::Solve(Arc::new(e)),
            SolveCondaEnvironmentError::SpecConversionError(e) => {
                SolveCondaKeyError::SpecConversion(Arc::new(e))
            }
            SolveCondaEnvironmentError::Gateway(e) => SolveCondaKeyError::Gateway(Arc::new(e)),
        }
    }
}

impl From<GatewayError> for SolveCondaKeyError {
    fn from(err: GatewayError) -> Self {
        SolveCondaKeyError::Gateway(Arc::new(err))
    }
}

impl From<SpecConversionError> for SolveCondaKeyError {
    fn from(err: SpecConversionError) -> Self {
        SolveCondaKeyError::SpecConversion(Arc::new(err))
    }
}

impl Key for SolveCondaKey {
    type Value = Result<Arc<Vec<PixiRecord>>, SolveCondaKeyError>;

    #[instrument(
        skip_all,
        name = "solve-conda",
        fields(platform = %self.0.platform, channels = self.0.channels.len())
    )]
    async fn compute(&self, ctx: &mut ComputeCtx) -> Self::Value {
        let spec = self.0.clone();

        // Collect every MatchSpec the solver could possibly need, so
        // the gateway fetch is recursive and complete.
        let channel_config = ctx.compute(&crate::ChannelConfigKey).await;

        let binary_match_specs = spec
            .binary_specs
            .clone()
            .into_match_specs(&channel_config)?;
        let constraint_match_specs = spec.constraints.clone().into_match_specs(&channel_config)?;

        // Seed the gateway fetch with every non-source dep mentioned
        // by the already-assembled source records and dev source
        // records we're about to feed to the solver. Their
        // `.depends` (post run-exports) is authoritative for which
        // binaries might actually need repodata entries.
        let source_repodata_fetch_specs = derive_fetch_specs_from_source_repodata(&spec);
        let dev_source_fetch_specs = derive_fetch_specs_from_dev_sources(&spec, &channel_config);

        // Clone the gateway handle so we don't hold an immutable
        // borrow on `ctx` across the subsequent mutable-borrow solve.
        let gateway = ctx.global_data().gateway().clone();
        let binary_repodata = gateway
            .query(
                spec.channels.iter().cloned().map(Channel::from_url),
                [spec.platform, Platform::NoArch],
                binary_match_specs
                    .into_iter()
                    .chain(constraint_match_specs)
                    .chain(source_repodata_fetch_specs)
                    .chain(dev_source_fetch_specs),
            )
            .recursive(true)
            .await?;

        // Build the full solve spec and hand off to ctx.solve_conda
        // (semaphore + reporter lifecycle).
        let conda_spec = SolveCondaEnvironmentSpec {
            name: None,
            source_specs: spec.source_specs.clone(),
            binary_specs: spec.binary_specs.clone(),
            constraints: spec.constraints.clone(),
            dev_source_records: spec.dev_source_records.clone(),
            source_repodata: spec.source_repodata.clone(),
            binary_repodata,
            installed: spec.installed.clone(),
            platform: spec.platform,
            channels: spec.channels.clone(),
            virtual_packages: spec.virtual_packages.clone(),
            strategy: spec.strategy,
            channel_priority: spec.channel_priority,
            exclude_newer: spec.exclude_newer.clone(),
        };

        let records = ctx.solve_conda(conda_spec).await?;
        Ok(Arc::new(records))
    }
}

/// For every assembled `SourceRecord` in `spec.source_repodata`,
/// emit a `MatchSpec` for each of its non-source depends. Source
/// deps are filtered via `record.sources()` since they're already
/// covered by `source_repodata`. Deriving specs here ensures the
/// fetch sees deps that only appear on a source record after
/// build/host run-exports have been merged. Unparsable depend
/// strings are skipped; the solver would reject them downstream
/// anyway.
fn derive_fetch_specs_from_source_repodata(spec: &SolveCondaSpec) -> Vec<MatchSpec> {
    let mut out = Vec::new();
    for sm in &spec.source_repodata {
        for record in &sm.records {
            let sources = record.sources();
            for depend in &record.package_record().depends {
                let Ok(match_spec) = MatchSpec::from_str(depend, ParseStrictness::Lenient) else {
                    continue;
                };
                if let PackageNameMatcher::Exact(ref name) = match_spec.name
                    && sources.contains_key(name.as_normalized())
                {
                    // Source dep; already in source_repodata.
                    continue;
                }
                out.push(match_spec);
            }
        }
    }
    out
}

/// Extract binary-typed match specs from the dependencies declared
/// by dev source records so the gateway fetch includes their
/// repodata. Source-typed deps are filtered out; they'd need
/// source_repodata entries that dev sources don't contribute.
fn derive_fetch_specs_from_dev_sources(
    spec: &SolveCondaSpec,
    channel_config: &ChannelConfig,
) -> Vec<MatchSpec> {
    let mut out = Vec::new();
    for dev_source in &spec.dev_source_records {
        for (name, pixi_spec) in dev_source.dependencies.iter_specs() {
            match pixi_spec.clone().into_source_or_binary() {
                Either::Right(binary) => {
                    if let Ok(nameless) = binary.try_into_nameless_match_spec(channel_config) {
                        out.push(MatchSpec::from_nameless(nameless, name.clone().into()));
                    }
                }
                Either::Left(_) => {
                    // Source dep from a dev source; not resolvable
                    // through the gateway.
                }
            }
        }
    }
    out
}
