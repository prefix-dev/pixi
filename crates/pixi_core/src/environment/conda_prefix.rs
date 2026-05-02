use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_once_cell::OnceCell as AsyncOnceCell;
use miette::IntoDiagnostic;
use pixi_command_dispatcher::{CommandDispatcher, InstallPixiEnvironmentSpec};
use pixi_compute_engine::{BuildEnvironment, EnvironmentFingerprint};
use pixi_manifest::FeaturesExt;
use pixi_record::{PixiRecord, UnresolvedPixiRecord};
use pixi_spec::ResolvedExcludeNewer;
use pixi_utils::{prefix::Prefix, variants::VariantConfig};
use rattler::install::link_script::LinkScriptType;
use rattler_conda_types::{
    ChannelUrl, GenericVirtualPackage, PackageName, Platform, RepoDataRecord,
};

use super::{
    conda_metadata::{create_history_file, create_prefix_location_file},
    try_increase_rlimit_to_sensible,
};
use crate::{
    environment::PythonStatus,
    workspace::{
        HasWorkspaceRef,
        grouped_environment::{GroupedEnvironment, GroupedEnvironmentName},
    },
};

/// The result of installing a conda prefix via [`update_prefix_conda`].
///
/// Contains the python status and the fully-resolved records for every
/// source package that was built during installation.
pub struct CondaPrefixInstallResult {
    /// Any change to the python interpreter.
    pub python_status: PythonStatus,

    /// For each source package that was built, the resulting binary record.
    /// Binary packages from the input are *not* included here.
    pub resolved_source_records: HashMap<PackageName, Arc<RepoDataRecord>>,

    /// Content fingerprint of every record now in the prefix; see
    /// [`pixi_compute_engine::EnvironmentFingerprint`].
    pub installed_fingerprint: EnvironmentFingerprint,
}

/// A struct that contains the result of updating a conda prefix.

#[derive(Clone)]
pub struct CondaPrefixUpdated {
    /// The name of the group that was updated.
    pub group: GroupedEnvironmentName,
    /// The prefix that was updated.
    pub prefix: Prefix,
    /// Any change to the python interpreter.
    pub python_status: Box<PythonStatus>,
    /// Fully-resolved records for source packages that were built.
    pub resolved_source_records: HashMap<PackageName, Arc<RepoDataRecord>>,
    /// Content fingerprint of every record now in the prefix; see
    /// [`pixi_compute_engine::EnvironmentFingerprint`].
    pub installed_fingerprint: EnvironmentFingerprint,
}

impl CondaPrefixUpdated {
    /// Merge unresolved records from the lock file with the build results
    /// to produce a fully-resolved set of [`PixiRecord`]s.
    ///
    /// Binary records pass through as-is. Source records are replaced by their
    /// built counterparts from [`resolved_source_records`](Self::resolved_source_records).
    pub fn into_pixi_records(self, unresolved: Vec<UnresolvedPixiRecord>) -> Vec<PixiRecord> {
        unresolved
            .into_iter()
            .filter_map(|r| match r {
                UnresolvedPixiRecord::Binary(b) => Some(PixiRecord::Binary(b)),
                UnresolvedPixiRecord::Source(_) => None,
            })
            .chain(
                self.resolved_source_records
                    .into_values()
                    .map(PixiRecord::Binary),
            )
            .collect()
    }
}

/// A task that updates the prefix for a given environment.
pub struct CondaPrefixUpdaterInner {
    pub channels: Vec<ChannelUrl>,
    pub name: GroupedEnvironmentName,
    pub prefix: Prefix,
    pub platform: Platform,
    pub virtual_packages: Vec<GenericVirtualPackage>,
    pub variant_config: VariantConfig,
    pub exclude_newer: Option<ResolvedExcludeNewer>,
    pub command_dispatcher: CommandDispatcher,

    /// A flag that indicates if the prefix was created.
    created: AsyncOnceCell<CondaPrefixUpdated>,
}

/// A builder for creating a new conda prefix updater.
pub struct CondaPrefixUpdaterBuilder<'a> {
    group: GroupedEnvironment<'a>,
    platform: Platform,
    virtual_packages: Vec<GenericVirtualPackage>,
    command_dispatcher: CommandDispatcher,
}

impl CondaPrefixUpdaterBuilder<'_> {
    /// Builds the conda prefix updater by extracting the necessary information
    /// from the group.
    pub fn finish(self) -> miette::Result<CondaPrefixUpdater> {
        let channels = self
            .group
            .channel_urls(&self.group.workspace().channel_config())
            .into_diagnostic()?;
        let name = self.group.name();
        let prefix = self.group.prefix();
        let variant_config = self.group.workspace().variants(self.platform)?;
        let exclude_newer = self
            .group
            .exclude_newer_config_resolved(&self.group.channel_config())
            .into_diagnostic()?;

        Ok(CondaPrefixUpdater::new(
            channels,
            name,
            prefix,
            self.platform,
            self.virtual_packages,
            variant_config,
            exclude_newer,
            self.command_dispatcher,
        ))
    }
}

#[derive(Clone)]
/// A task that updates the prefix for a given environment.
pub struct CondaPrefixUpdater {
    inner: Arc<CondaPrefixUpdaterInner>,
}

impl CondaPrefixUpdater {
    /// Constructs a builder.
    pub fn builder(
        group: GroupedEnvironment<'_>,
        platform: Platform,
        virtual_packages: Vec<GenericVirtualPackage>,
        command_dispatcher: CommandDispatcher,
    ) -> CondaPrefixUpdaterBuilder<'_> {
        CondaPrefixUpdaterBuilder {
            group,
            platform,
            virtual_packages,
            command_dispatcher,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        channels: Vec<ChannelUrl>,
        name: GroupedEnvironmentName,
        prefix: Prefix,
        platform: Platform,
        virtual_packages: Vec<GenericVirtualPackage>,
        variant_config: VariantConfig,
        exclude_newer: Option<ResolvedExcludeNewer>,
        command_dispatcher: CommandDispatcher,
    ) -> Self {
        Self {
            inner: Arc::new(CondaPrefixUpdaterInner {
                channels,
                name,
                prefix,
                platform,
                virtual_packages,
                variant_config,
                exclude_newer,
                command_dispatcher,
                created: Default::default(),
            }),
        }
    }

    /// Updates the prefix for the given environment.
    pub async fn update(
        &self,
        pixi_records: Vec<UnresolvedPixiRecord>,
        reinstall_packages: Option<HashSet<PackageName>>,
        ignore_packages: Option<HashSet<PackageName>>,
    ) -> miette::Result<&CondaPrefixUpdated> {
        self.inner
            .created
            .get_or_try_init(async {
                tracing::debug!("updating prefix for '{}'", self.inner.name.fancy_display());

                let channels = self.inner.channels.clone();

                let group_name = self.inner.name.clone();

                let install_result = update_prefix_conda(
                    self.name().to_string(),
                    &self.inner.prefix,
                    pixi_records,
                    channels,
                    self.inner.platform,
                    self.inner.virtual_packages.clone(),
                    self.inner.variant_config.clone(),
                    self.inner.exclude_newer.clone(),
                    self.inner.command_dispatcher.clone(),
                    reinstall_packages,
                    ignore_packages,
                )
                .await?;

                Ok(CondaPrefixUpdated {
                    group: group_name,
                    prefix: self.inner.prefix.clone(),
                    python_status: Box::new(install_result.python_status),
                    resolved_source_records: install_result.resolved_source_records,
                    installed_fingerprint: install_result.installed_fingerprint,
                })
            })
            .await
    }

    pub(crate) fn name(&self) -> &GroupedEnvironmentName {
        &self.inner.name
    }
}

/// Updates the environment to contain the packages from the specified lock-file
#[allow(clippy::too_many_arguments)]
pub async fn update_prefix_conda(
    name: String,
    prefix: &Prefix,
    pixi_records: Vec<UnresolvedPixiRecord>,
    channels: Vec<ChannelUrl>,
    host_platform: Platform,
    host_virtual_packages: Vec<GenericVirtualPackage>,
    variant_config: VariantConfig,
    exclude_newer: Option<ResolvedExcludeNewer>,
    command_dispatcher: CommandDispatcher,
    reinstall_packages: Option<HashSet<PackageName>>,
    ignore_packages: Option<HashSet<PackageName>>,
) -> miette::Result<CondaPrefixInstallResult> {
    // Try to increase the rlimit to a sensible value for installation.
    try_increase_rlimit_to_sensible();

    // Run the installation through the command dispatcher.
    let build_environment = BuildEnvironment::simple(host_platform, host_virtual_packages);
    let VariantConfig {
        variant_configuration,
        variant_files,
    } = variant_config;
    let force_reinstall = reinstall_packages.unwrap_or_default();

    // Force-reinstall also invalidates the source-build caches. The
    // prefix installer handles binary reinstalls itself; source
    // packages need their artifact + workspace entries wiped so
    // SourceBuildKey sees a cache miss. clear_source_build_cache is a
    // no-op on packages that were never built from source.
    for name in &force_reinstall {
        command_dispatcher
            .clear_source_build_cache(name)
            .into_diagnostic()?;
    }

    let result = command_dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            name,
            records: pixi_records,
            prefix: rattler_conda_types::prefix::Prefix::create(prefix.root()).into_diagnostic()?,
            installed: None,
            force_reinstall,
            ignore_packages,
            build_environment,
            exclude_newer,
            channels,
            variant_configuration: Some(variant_configuration),
            variant_files: Some(variant_files),
        })
        .await?;

    // Mark the location of the prefix
    create_prefix_location_file(prefix.root())?;
    create_history_file(prefix.root())?;

    // Check in the prefix if there are any `post-link` scripts that have not been
    // executed, and if yes, issue a one-time warning to the user.
    if !command_dispatcher.allow_execute_link_scripts() {
        let mut skipped_scripts = Vec::new();

        for package in result.transaction.installed_packages() {
            let rel_script_path =
                LinkScriptType::PreUnlink.get_path(&package.package_record, &host_platform);
            let post_link_script = prefix.root().join(&rel_script_path);

            if post_link_script.exists() {
                skipped_scripts.push(rel_script_path);
            }
        }

        if !skipped_scripts.is_empty() {
            let script_list = skipped_scripts
                .iter()
                .map(|p| format!("\t- {}", console::style(p).yellow()))
                .collect::<Vec<_>>()
                .join("\n");

            tracing::warn!(
                "Skipped running the post-link scripts because `{}` = `{}`\n\
            {}\n\n\
            To enable them, run:\n\
            \t{}\n\n\
            More info:\n\
            \thttps://pixi.sh/latest/reference/pixi_configuration/#run-post-link-scripts\n",
                console::style("run-post-link-scripts").bold(),
                console::style("false").cyan(),
                script_list,
                console::style("pixi config set --local run-post-link-scripts insecure").green(),
            );
        }
    }

    // Determine if the python version changed.
    let python_status = PythonStatus::from_transaction(&result.transaction);

    Ok(CondaPrefixInstallResult {
        python_status,
        resolved_source_records: result.resolved_source_records,
        installed_fingerprint: result.installed_fingerprint,
    })
}
