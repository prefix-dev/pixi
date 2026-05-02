//! Binary-only `install_pixi_environment` primitive on
//! [`ComputeCtx`](crate::ComputeCtx).
//!
//! Lays down a pixi environment from a fully-resolved set of binary
//! [`RepoDataRecord`]s using rattler's prefix [`Installer`]. Source
//! packages are not handled here: callers that need to install a
//! mixed source/binary set must build the source records into binary
//! ones upstream and pass the merged list in.
//!
//! What lives here, what doesn't:
//!
//! - **Here:** the rattler-installer call, the [`EnvironmentFingerprint`]
//!   short-circuit on unchanged prefixes, the rattler-level
//!   [`rattler::install::Reporter`] pass-through wrapper.
//! - **Not here:** the outer "install lifecycle" reporter frame
//!   (queued/started/finished + [`ReporterContext`] scoping for nested
//!   operations) and any source-build fanout. Both stay in
//!   `pixi_command_dispatcher`, which wraps this primitive.

use std::{collections::HashSet, ffi::OsStr, sync::Arc};

use miette::Diagnostic;
use rattler::install::{
    InstallationResultRecord, Installer, InstallerError, PythonInfo, Transaction,
    link_script::{LinkScriptError, PrePostLinkResult},
};
use rattler::package_cache::PackageCache;
use rattler_conda_types::{PackageName, Platform, PrefixRecord, RepoDataRecord, prefix::Prefix};
use rattler_networking::LazyClient;
use thiserror::Error;

use crate::{BuildEnvironment, ComputeCtx, DataStore, EnvironmentFingerprint};

/// Wrapper newtype so a `bool` flag can live in
/// [`DataStore`] keyed by its own `TypeId` without colliding with
/// other `bool`s.
#[derive(Copy, Clone, Debug)]
pub struct AllowExecuteLinkScripts(pub bool);

/// Spec for a binary-only pixi environment install.
///
/// All `records` must already be resolved to binary
/// [`RepoDataRecord`]s. Mixed source/binary specs are the caller's
/// responsibility to flatten before they reach this primitive.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallPixiEnvironmentSpec {
    pub name: String,

    /// Fully-resolved binary records to install.
    #[serde(skip)]
    pub records: Vec<Arc<RepoDataRecord>>,

    /// Packages neither removed when missing from `records` nor updated
    /// when already installed.
    pub ignore_packages: Option<HashSet<PackageName>>,

    #[serde(skip)]
    pub prefix: Prefix,

    #[serde(skip)]
    pub installed: Option<Vec<PrefixRecord>>,

    pub build_environment: BuildEnvironment,

    #[serde(skip_serializing_if = "HashSet::is_empty")]
    pub force_reinstall: HashSet<PackageName>,
}

impl InstallPixiEnvironmentSpec {
    /// Construct a spec from a list of resolved binary records and a
    /// target prefix. Other fields take their defaults; mutate them
    /// directly on the returned struct as needed.
    pub fn new(records: Vec<Arc<RepoDataRecord>>, prefix: Prefix) -> Self {
        InstallPixiEnvironmentSpec {
            name: prefix
                .file_name()
                .map(OsStr::to_string_lossy)
                .map(std::borrow::Cow::into_owned)
                .unwrap_or_default(),
            records,
            prefix,
            installed: None,
            ignore_packages: None,
            build_environment: BuildEnvironment::default(),
            force_reinstall: HashSet::new(),
        }
    }
}

/// Outcome of a [`InstallPixiEnvironmentExt::install_pixi_environment`]
/// call.
pub struct InstallPixiEnvironmentResult {
    pub transaction: Transaction<InstallationResultRecord, RepoDataRecord>,

    /// `None` when link scripts were disabled.
    pub pre_link_script_result: Option<PrePostLinkResult>,

    /// `None` when link scripts were disabled.
    pub post_link_script_result: Option<Result<PrePostLinkResult, LinkScriptError>>,

    /// Content fingerprint of every record that ended up in the
    /// prefix; see [`EnvironmentFingerprint`].
    pub installed_fingerprint: EnvironmentFingerprint,
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstallPixiEnvironmentError {
    #[error("failed to collect prefix records from '{}'", .0.path().display())]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ReadInstalledPackages(Prefix, #[source] std::io::Error),

    #[error(transparent)]
    Installer(InstallerError),

    #[error(
        "failed to convert install transaction to prefix records from '{}'",
        .0.path().display()
    )]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ConvertTransactionToPrefixRecord(Prefix, #[source] std::io::Error),

    #[error("failed to determine python info for the installed environment: {0}")]
    DetectPythonInfo(String),
}

/// Pass-through wrapper so a boxed [`rattler::install::Reporter`] can
/// be passed through [`Installer::with_reporter`], which takes
/// `impl Reporter` rather than `Box<dyn Reporter>`.
pub struct WrappingInstallReporter(pub Box<dyn rattler::install::Reporter>);

impl rattler::install::Reporter for WrappingInstallReporter {
    fn on_transaction_start(&self, transaction: &Transaction<PrefixRecord, RepoDataRecord>) {
        self.0.on_transaction_start(transaction)
    }

    fn on_transaction_operation_start(&self, operation: usize) {
        self.0.on_transaction_operation_start(operation)
    }

    fn on_populate_cache_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        self.0.on_populate_cache_start(operation, record)
    }

    fn on_validate_start(&self, cache_entry: usize) -> usize {
        self.0.on_validate_start(cache_entry)
    }

    fn on_validate_complete(&self, validate_idx: usize) {
        self.0.on_validate_complete(validate_idx)
    }

    fn on_download_start(&self, cache_entry: usize) -> usize {
        self.0.on_download_start(cache_entry)
    }

    fn on_download_progress(&self, download_idx: usize, progress: u64, total: Option<u64>) {
        self.0.on_download_progress(download_idx, progress, total)
    }

    fn on_download_completed(&self, download_idx: usize) {
        self.0.on_download_completed(download_idx)
    }

    fn on_populate_cache_complete(&self, cache_entry: usize) {
        self.0.on_populate_cache_complete(cache_entry)
    }

    fn on_unlink_start(&self, operation: usize, record: &PrefixRecord) -> usize {
        self.0.on_unlink_start(operation, record)
    }

    fn on_unlink_complete(&self, index: usize) {
        self.0.on_unlink_complete(index)
    }

    fn on_link_start(&self, operation: usize, record: &RepoDataRecord) -> usize {
        self.0.on_link_start(operation, record)
    }

    fn on_link_complete(&self, index: usize) {
        self.0.on_link_complete(index)
    }

    fn on_post_link_start(&self, package_name: &str, script_path: &str) -> usize {
        self.0.on_post_link_start(package_name, script_path)
    }

    fn on_post_link_complete(&self, index: usize, success: bool) {
        self.0.on_post_link_complete(index, success)
    }

    fn on_pre_unlink_start(&self, package_name: &str, script_path: &str) -> usize {
        self.0.on_pre_unlink_start(package_name, script_path)
    }

    fn on_pre_unlink_complete(&self, index: usize, success: bool) {
        self.0.on_pre_unlink_complete(index, success)
    }

    fn on_transaction_operation_complete(&self, operation: usize) {
        self.0.on_transaction_operation_complete(operation)
    }

    fn on_transaction_complete(&self) {
        self.0.on_transaction_complete()
    }
}

/// Extension trait on [`ComputeCtx`] that runs a binary-only pixi
/// environment install through rattler's prefix installer.
pub trait InstallPixiEnvironmentExt {
    /// Install the resolved binary records described by `spec` into
    /// `spec.prefix`. The optional `install_reporter` is the
    /// rattler-level reporter; pixi-level lifecycle reporting (queued
    /// / started / finished, nested-operation [`ReporterContext`]
    /// scoping) is the caller's responsibility.
    fn install_pixi_environment(
        &mut self,
        spec: InstallPixiEnvironmentSpec,
        install_reporter: Option<Box<dyn rattler::install::Reporter>>,
    ) -> impl Future<Output = Result<InstallPixiEnvironmentResult, InstallPixiEnvironmentError>> + Send;
}

impl InstallPixiEnvironmentExt for ComputeCtx {
    async fn install_pixi_environment(
        &mut self,
        spec: InstallPixiEnvironmentSpec,
        install_reporter: Option<Box<dyn rattler::install::Reporter>>,
    ) -> Result<InstallPixiEnvironmentResult, InstallPixiEnvironmentError> {
        install_inner(self, spec, install_reporter).await
    }
}

async fn install_inner(
    ctx: &mut ComputeCtx,
    mut spec: InstallPixiEnvironmentSpec,
    install_reporter: Option<Box<dyn rattler::install::Reporter>>,
) -> Result<InstallPixiEnvironmentResult, InstallPixiEnvironmentError> {
    let binary_records = std::mem::take(&mut spec.records);

    // Fingerprint every record that will land in the prefix; the
    // sha256s the records already carry are enough, no file I/O.
    let installed_fingerprint =
        EnvironmentFingerprint::compute(binary_records.iter().map(|arc| arc.as_ref()));

    // Fast path: when the prefix's stored fingerprint already matches
    // the one we'd install and the caller hasn't asked for an explicit
    // reinstall, skip the rattler installer entirely.
    if spec.force_reinstall.is_empty()
        && EnvironmentFingerprint::read(spec.prefix.path()).as_ref() == Some(&installed_fingerprint)
    {
        let transaction =
            unchanged_transaction(spec.build_environment.host_platform, &binary_records)?;
        return Ok(InstallPixiEnvironmentResult {
            transaction,
            post_link_script_result: None,
            pre_link_script_result: None,
            installed_fingerprint,
        });
    }

    // Run the rattler prefix installer against the fully-resolved binary
    // set. Resources come from the compute engine's DataStore.
    let data: &DataStore = ctx.global_data();
    let download_client = data.get::<LazyClient>();
    let package_cache = data.get::<PackageCache>();
    let allow_link_scripts = data
        .try_get::<AllowExecuteLinkScripts>()
        .map(|v| v.0)
        .unwrap_or(false);

    let mut installer = Installer::new()
        .with_target_platform(spec.build_environment.host_platform)
        .with_download_client(download_client.clone())
        .with_package_cache(package_cache.clone())
        .with_reinstall_packages(std::mem::take(&mut spec.force_reinstall))
        .with_ignored_packages(spec.ignore_packages.take().unwrap_or_default())
        .with_execute_link_scripts(allow_link_scripts);
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
        })?;

    Ok(InstallPixiEnvironmentResult {
        transaction: result.transaction,
        post_link_script_result: result.post_link_script_result,
        pre_link_script_result: result.pre_link_script_result,
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
fn unchanged_transaction(
    platform: Platform,
    records: &[Arc<RepoDataRecord>],
) -> Result<Transaction<InstallationResultRecord, RepoDataRecord>, InstallPixiEnvironmentError> {
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
