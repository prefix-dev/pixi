//! `ctx.install_pixi_environment` extension trait. Installs a pixi
//! environment by (a) concurrently building every source record via
//! [`SourceBuildKey`], then (b) delegating the binary install to
//! `pixi_compute_engine`'s prefix-install primitive.
//!
//! Reporter lifecycle (queued/started/finished + nested-context
//! scoping) and source-build orchestration live here; the actual
//! prefix-installer call lives in `pixi_compute_engine::install_pixi`.

use std::{collections::HashMap, sync::Arc};

use pixi_compute_engine::{BuildEnvironment, ComputeCtx};
use pixi_record::UnresolvedPixiRecord;
use rattler_conda_types::{PackageName, RepoDataRecord};

use crate::BuildProfile;
use crate::CommandDispatcherError;
use crate::compute_data::{HasCacheDirs, HasReporter};
use crate::install_pixi::{
    InstallPixiEnvironmentError, InstallPixiEnvironmentResult, InstallPixiEnvironmentSpec,
};
use crate::keys::{ArtifactCache, SourceBuildKey, SourceBuildSpecV2, WorkspaceCache};
use crate::reporter::{Reporter, ReporterContext};
use crate::reporter_context::{CURRENT_REPORTER_CONTEXT, current_reporter_context};

/// Extension trait on [`ComputeCtx`] that installs a pixi environment
/// with source-build recursion routed through [`SourceBuildKey`].
pub trait InstallPixiEnvironmentExt {
    fn install_pixi_environment(
        &mut self,
        spec: InstallPixiEnvironmentSpec,
    ) -> impl Future<
        Output = Result<
            InstallPixiEnvironmentResult,
            CommandDispatcherError<InstallPixiEnvironmentError>,
        >,
    > + Send;
}

impl InstallPixiEnvironmentExt for ComputeCtx {
    async fn install_pixi_environment(
        &mut self,
        spec: InstallPixiEnvironmentSpec,
    ) -> Result<InstallPixiEnvironmentResult, CommandDispatcherError<InstallPixiEnvironmentError>>
    {
        // Reporter lifecycle for this pixi-install frame. Queue up-front
        // so the reporter can count us against the parent context before
        // any source builds fan out.
        let reporter_arc = self.global_data().reporter().cloned();
        let parent_reporter_ctx = current_reporter_context();
        let reporter_fn = || {
            reporter_arc
                .as_deref()
                .and_then(Reporter::as_pixi_install_reporter)
        };
        let reporter_id = reporter_fn().map(|r| r.on_queued(parent_reporter_ctx, &spec));
        if let (Some(r), Some(id)) = (reporter_fn(), reporter_id) {
            r.on_started(id);
        }

        // Build the rattler install reporter under the *parent* context
        // so prefix-install events (validate/download/link) nest correctly
        // alongside our own lifecycle events.
        let install_reporter = reporter_arc
            .as_deref()
            .and_then(|r| r.create_install_reporter(parent_reporter_ctx));

        // Scope child reporter context to this install so source builds
        // fanned out below attribute their progress to us.
        let scope_ctx = reporter_id
            .map(ReporterContext::InstallPixi)
            .or(parent_reporter_ctx);
        let work = install_inner(self, spec, install_reporter);
        let result = match scope_ctx {
            Some(rc) => CURRENT_REPORTER_CONTEXT.scope(Some(rc), work).await,
            None => work.await,
        };

        if let (Some(r), Some(id)) = (reporter_fn(), reporter_id) {
            r.on_finished(id);
        }

        result
    }
}

/// Shared source-build parameters that do not vary across the records
/// being built together in one install call. Cloned cheaply into each
/// [`try_compute_join`](ComputeCtx::try_compute_join) branch.
#[derive(Clone)]
struct SharedBuildParams {
    channels: Vec<rattler_conda_types::ChannelUrl>,
    exclude_newer: Option<pixi_spec::ResolvedExcludeNewer>,
    build_environment: BuildEnvironment,
    variant_configuration:
        Option<std::collections::BTreeMap<String, Vec<pixi_record::VariantValue>>>,
    variant_files: Option<Vec<std::path::PathBuf>>,
}

async fn install_inner(
    ctx: &mut ComputeCtx,
    mut spec: InstallPixiEnvironmentSpec,
    install_reporter: Option<Box<dyn rattler::install::Reporter>>,
) -> Result<InstallPixiEnvironmentResult, CommandDispatcherError<InstallPixiEnvironmentError>> {
    // Split into source and binary records up front. Ignored source
    // packages are dropped so they never drive a SourceBuildKey.
    let mut source_records: Vec<Arc<pixi_record::UnresolvedSourceRecord>> =
        Vec::with_capacity(spec.records.len() / 2);
    let mut binary_records: Vec<Arc<RepoDataRecord>> = Vec::with_capacity(spec.records.len());
    for record in std::mem::take(&mut spec.records) {
        match record {
            UnresolvedPixiRecord::Source(r) => {
                if !spec
                    .ignore_packages
                    .as_ref()
                    .is_some_and(|set| set.contains(r.name()))
                {
                    source_records.push(r);
                }
            }
            UnresolvedPixiRecord::Binary(r) => binary_records.push(r),
        }
    }

    // `force_reinstall` for source packages must invalidate the
    // source-build caches before the SourceBuildKey fanout below.
    if !spec.force_reinstall.is_empty() {
        let cache_dirs = ctx.global_data().cache_dirs();
        let artifact_cache = ArtifactCache::new(cache_dirs.source_build_artifacts().as_std_path());
        let workspace_cache =
            WorkspaceCache::new(cache_dirs.source_build_workspaces().as_std_path());
        for package in source_records
            .iter()
            .filter(|record| spec.force_reinstall.contains(record.name()))
            .map(|record| record.name())
        {
            artifact_cache
                .clear_package(package)
                .and_then(|()| workspace_cache.clear_package(package))
                .map_err(|err| {
                    CommandDispatcherError::Failed(
                        InstallPixiEnvironmentError::ClearSourceBuildCache(package.clone(), err),
                    )
                })?;
        }
    }

    // Build source packages concurrently via SourceBuildKey. Each branch
    // gets a sub-ctx; `try_compute_join` short-circuits on the first
    // error.
    let shared = SharedBuildParams {
        channels: spec.channels.clone(),
        exclude_newer: spec.exclude_newer.clone(),
        build_environment: spec.build_environment.clone(),
        variant_configuration: spec.variant_configuration.clone(),
        variant_files: spec.variant_files.clone(),
    };
    let mapper = {
        let shared = shared.clone();
        async move |sub_ctx: &mut ComputeCtx,
                    source: Arc<pixi_record::UnresolvedSourceRecord>|
                    -> Result<
            Arc<crate::keys::source_build::SourceBuildResult>,
            InstallPixiEnvironmentError,
        > {
            let name = source.name().clone();
            let manifest_source = source.manifest_source.clone();
            let build_spec = SourceBuildSpecV2 {
                record: source,
                channels: shared.channels.clone(),
                exclude_newer: shared.exclude_newer.clone(),
                build_environment: shared.build_environment.clone(),
                // Installing a pixi environment always builds in
                // development mode.
                build_profile: BuildProfile::Development,
                variant_configuration: shared.variant_configuration.clone(),
                variant_files: shared.variant_files.clone(),
            };
            sub_ctx
                .compute(&SourceBuildKey::new(build_spec))
                .await
                .map_err(|err| {
                    InstallPixiEnvironmentError::BuildUnresolvedSourceError(
                        name,
                        Box::new(manifest_source),
                        err,
                    )
                })
        }
    };
    let built_sources = ctx
        .try_compute_join(source_records, mapper)
        .await
        .map_err(CommandDispatcherError::Failed)?;

    // Merge built source records into the binary set and keep a lookup
    // map so callers (e.g. build-prefix assemblers) can find the
    // RepoDataRecord for each source package by name.
    let mut resolved_source_records: HashMap<PackageName, Arc<RepoDataRecord>> = HashMap::new();
    for built in built_sources {
        let record = Arc::new(built.record.clone());
        resolved_source_records.insert(record.package_record.name.clone(), record.clone());
        binary_records.push(record);
    }

    // Delegate the binary install to compute_engine. The engine
    // primitive computes the fingerprint, short-circuits on a match,
    // and otherwise drives the rattler prefix installer. Spelled-out
    // UFCS because `ComputeCtx` carries both `InstallPixiEnvironmentExt`
    // impls (this crate's mixed-records one and the engine's binary-only
    // one) with the same method name.
    let engine_spec = pixi_compute_engine::InstallPixiEnvironmentSpec {
        name: spec.name.clone(),
        records: binary_records,
        ignore_packages: spec.ignore_packages.take(),
        prefix: spec.prefix.clone(),
        installed: spec.installed.take(),
        build_environment: spec.build_environment.clone(),
        force_reinstall: std::mem::take(&mut spec.force_reinstall),
    };
    let engine_result =
        <ComputeCtx as pixi_compute_engine::InstallPixiEnvironmentExt>::install_pixi_environment(
            ctx,
            engine_spec,
            install_reporter,
        )
        .await
        .map_err(|err| CommandDispatcherError::Failed(err.into()))?;

    Ok(InstallPixiEnvironmentResult {
        transaction: engine_result.transaction,
        post_link_script_result: engine_result.post_link_script_result,
        pre_link_script_result: engine_result.pre_link_script_result,
        resolved_source_records,
        installed_fingerprint: engine_result.installed_fingerprint,
    })
}
