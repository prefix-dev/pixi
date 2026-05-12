//! `ctx.install_pixi_environment` extension trait. Installs a pixi
//! environment by (a) concurrently building every source record via
//! [`SourceBuildKey`], then (b) running the
//! rattler prefix installer over the resulting binary set.

use std::{collections::HashMap, sync::Arc};

use pixi_compute_engine::{ComputeCtx, DataStore};
use pixi_record::UnresolvedPixiRecord;
use rattler::install::{Installer, InstallerError, PythonInfo, Transaction};
use rattler_conda_types::{PackageName, RepoDataRecord};

use crate::BuildProfile;
use crate::CommandDispatcherError;
use crate::cache::markers::{SourceBuildArtifactsDir, SourceBuildWorkspacesDir};
use crate::compute_data::{
    HasAllowExecuteLinkScripts, HasAllowLinkOptions, HasPackageCache, HasPixiInstallReporter,
};
use crate::install_pixi::{
    InstallPixiEnvironmentError, InstallPixiEnvironmentResult, InstallPixiEnvironmentSpec,
    reporter::WrappingInstallReporter,
};
use crate::keys::{ArtifactCache, SourceBuildKey, SourceBuildSpec, WorkspaceCache};
use crate::reporter::PixiInstallReporter;
use pixi_compute_cache_dirs::CacheDirsExt;
use pixi_compute_network::HasDownloadClient;

/// Extension trait on [`ComputeCtx`] that installs a pixi environment
/// with source-build recursion routed through [`SourceBuildKey`].
pub trait InstallPixiEnvironmentExt {
    /// Reports progress via `Arc<dyn PixiInstallReporter>` set on the engine `DataStore`, if any.
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
        // so the reporter can count us before any source builds fan out.
        let pixi_install_reporter = self.global_data().pixi_install_reporter().cloned();
        let reporter_id = pixi_install_reporter.as_deref().map(|r| r.on_queued(&spec));
        if let (Some(r), Some(id)) = (pixi_install_reporter.as_deref(), reporter_id) {
            r.on_started(id);
        }

        // Build the rattler install reporter; it nests under the
        // currently-active reporter context.
        let install_reporter = pixi_install_reporter
            .as_deref()
            .and_then(PixiInstallReporter::create_install_reporter);

        // Scope source builds fanned out below under this install's id.
        let work = install_inner(self, spec, install_reporter);
        let result = match reporter_id {
            Some(id) => id.scope_active(work).await,
            None => work.await,
        };

        if let (Some(r), Some(id)) = (pixi_install_reporter.as_deref(), reporter_id) {
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
    build_environment: crate::BuildEnvironment,
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
        let artifacts_dir = ctx.cache_dir::<SourceBuildArtifactsDir>().await;
        let workspaces_dir = ctx.cache_dir::<SourceBuildWorkspacesDir>().await;
        let artifact_cache = ArtifactCache::new(artifacts_dir.as_std_path());
        let workspace_cache = WorkspaceCache::new(workspaces_dir.as_std_path());
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
            let build_spec = SourceBuildSpec {
                record: source,
                channels: shared.channels.clone(),
                exclude_newer: shared.exclude_newer.clone(),
                build_environment: shared.build_environment.clone(),
                // Installing a pixi environment always builds in
                // development mode.
                build_profile: BuildProfile::Development,
                variant_configuration: shared.variant_configuration.clone(),
                variant_files: shared.variant_files.clone(),
                // `pixi install` does not expose CLI-level overrides for
                // build_string_prefix / build_number; only `pixi publish`
                // forwards user-supplied values here.
                build_string_prefix: None,
                build_number: None,
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

    // Fingerprint every record that will land in the prefix; the
    // sha256s the records already carry are enough, no file I/O.
    let installed_fingerprint =
        crate::EnvironmentFingerprint::compute(binary_records.iter().map(|arc| arc.as_ref()));

    // Fast path: when the prefix's stored fingerprint already matches
    // the one we'd install and the caller hasn't asked for an explicit
    // reinstall, skip the rattler installer entirely. Source builds
    // above this point still ran (their content feeds the
    // fingerprint), so any source change forces a fresh install via a
    // fingerprint mismatch.
    if spec.force_reinstall.is_empty()
        && crate::EnvironmentFingerprint::read(spec.prefix.path()).as_ref()
            == Some(&installed_fingerprint)
    {
        let transaction =
            unchanged_transaction(spec.build_environment.host_platform, &binary_records)
                .map_err(CommandDispatcherError::Failed)?;
        return Ok(InstallPixiEnvironmentResult {
            transaction,
            post_link_script_result: None,
            pre_link_script_result: None,
            resolved_source_records,
            installed_fingerprint,
        });
    }

    // Run the rattler prefix installer against the fully-resolved binary
    // set. Resources come from the compute engine's DataStore.
    let data: &DataStore = ctx.global_data();
    let mut installer = Installer::new()
        .with_target_platform(spec.build_environment.host_platform)
        .with_download_client(data.download_client().clone())
        .with_package_cache(data.package_cache().clone())
        .with_reinstall_packages(std::mem::take(&mut spec.force_reinstall))
        .with_ignored_packages(spec.ignore_packages.take().unwrap_or_default())
        .with_execute_link_scripts(data.allow_execute_link_scripts())
        .with_link_options(data.allow_link_options());
    if let Some(installed) = spec.installed.take() {
        installer = installer.with_installed_packages(installed);
    }
    if let Some(reporter) = install_reporter {
        installer = installer.with_reporter(WrappingInstallReporter(reporter));
    }

    let result = installer
        .install(
            spec.prefix.path(),
            binary_records.into_iter().map(Arc::unwrap_or_clone),
        )
        .await
        .map_err(|err| match err {
            InstallerError::FailedToDetectInstalledPackages(err) => {
                InstallPixiEnvironmentError::ReadInstalledPackages(spec.prefix.clone(), err)
            }
            err => InstallPixiEnvironmentError::Installer(err),
        })
        .map_err(CommandDispatcherError::Failed)?;

    Ok(InstallPixiEnvironmentResult {
        transaction: result.transaction,
        post_link_script_result: result.post_link_script_result,
        pre_link_script_result: result.pre_link_script_result,
        resolved_source_records,
        installed_fingerprint,
    })
}

/// Build the [`Transaction`] returned to the caller when the install
/// short-circuits on a fingerprint match. There's no work to perform,
/// so `operations` is empty; only `python_info` and
/// `current_python_info` need real values so downstream code that
/// derives `PythonStatus` from the transaction sees `Unchanged`. We
/// leave `unchanged` empty too: callers iterate it to inspect the
/// install diff, and "no diff" is exactly what we want to signal.
#[allow(clippy::result_large_err)] // matches install_inner's unboxed error contract
fn unchanged_transaction(
    platform: rattler_conda_types::Platform,
    records: &[Arc<RepoDataRecord>],
) -> Result<
    Transaction<rattler::install::InstallationResultRecord, RepoDataRecord>,
    InstallPixiEnvironmentError,
> {
    let python_info = records
        .iter()
        .find(|r| r.package_record.name.as_normalized() == "python")
        .map(|r| PythonInfo::from_python_record(&r.package_record, platform))
        .transpose()
        .map_err(|err| InstallPixiEnvironmentError::DetectPythonInfo(err.to_string()))?;
    Ok(Transaction {
        operations: Vec::new(),
        python_info: python_info.clone(),
        current_python_info: python_info,
        platform,
        unchanged: Vec::new(),
    })
}
