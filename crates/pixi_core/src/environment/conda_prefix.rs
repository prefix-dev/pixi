use std::{collections::HashSet, sync::Arc};

use async_once_cell::OnceCell as AsyncOnceCell;
use miette::IntoDiagnostic;
use pixi_command_dispatcher::{BuildEnvironment, CommandDispatcher, InstallPixiEnvironmentSpec};
use pixi_manifest::FeaturesExt;
use pixi_record::PixiRecord;
use pixi_utils::{prefix::Prefix, variants::VariantConfig};
use rattler::install::link_script::LinkScriptType;
use rattler_conda_types::{
    ChannelConfig, ChannelUrl, GenericVirtualPackage, PackageName, Platform,
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

/// A struct that contains the result of updating a conda prefix.

#[derive(Clone)]
pub struct CondaPrefixUpdated {
    /// The name of the group that was updated.
    pub group: GroupedEnvironmentName,
    /// The prefix that was updated.
    pub prefix: Prefix,
    /// Any change to the python interpreter.
    pub python_status: Box<PythonStatus>,
}

/// A task that updates the prefix for a given environment.
pub struct CondaPrefixUpdaterInner {
    pub channels: Vec<ChannelUrl>,
    pub channel_config: ChannelConfig,
    pub name: GroupedEnvironmentName,
    pub prefix: Prefix,
    pub platform: Platform,
    pub virtual_packages: Vec<GenericVirtualPackage>,
    pub variant_config: VariantConfig,
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

        Ok(CondaPrefixUpdater::new(
            channels,
            self.group.channel_config(),
            name,
            prefix,
            self.platform,
            self.virtual_packages,
            self.group.workspace().variants(self.platform),
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
        channel_config: ChannelConfig,
        name: GroupedEnvironmentName,
        prefix: Prefix,
        platform: Platform,
        virtual_packages: Vec<GenericVirtualPackage>,
        variant_config: VariantConfig,
        command_dispatcher: CommandDispatcher,
    ) -> Self {
        Self {
            inner: Arc::new(CondaPrefixUpdaterInner {
                channels,
                channel_config,
                name,
                prefix,
                platform,
                virtual_packages,
                variant_config,
                command_dispatcher,
                created: Default::default(),
            }),
        }
    }

    /// Updates the prefix for the given environment.
    pub async fn update(
        &self,
        pixi_records: Vec<PixiRecord>,
        reinstall_packages: Option<HashSet<PackageName>>,
        ignore_packages: Option<HashSet<PackageName>>,
    ) -> miette::Result<&CondaPrefixUpdated> {
        self.inner
            .created
            .get_or_try_init(async {
                tracing::debug!("updating prefix for '{}'", self.inner.name.fancy_display());

                let channels = self.inner.channels.clone();

                let group_name = self.inner.name.clone();

                let python_status = update_prefix_conda(
                    self.name().to_string(),
                    &self.inner.prefix,
                    pixi_records,
                    channels,
                    self.inner.channel_config.clone(),
                    self.inner.platform,
                    self.inner.virtual_packages.clone(),
                    self.inner.variant_config.clone(),
                    self.inner.command_dispatcher.clone(),
                    reinstall_packages,
                    ignore_packages,
                )
                .await?;

                Ok(CondaPrefixUpdated {
                    group: group_name,
                    prefix: self.inner.prefix.clone(),
                    python_status: Box::new(python_status),
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
    pixi_records: Vec<PixiRecord>,
    channels: Vec<ChannelUrl>,
    channel_config: ChannelConfig,
    host_platform: Platform,
    host_virtual_packages: Vec<GenericVirtualPackage>,
    variant_config: VariantConfig,
    command_dispatcher: CommandDispatcher,
    reinstall_packages: Option<HashSet<PackageName>>,
    ignore_packages: Option<HashSet<PackageName>>,
) -> miette::Result<PythonStatus> {
    // Try to increase the rlimit to a sensible value for installation.
    try_increase_rlimit_to_sensible();

    // Run the installation through the command dispatcher.
    let build_environment = BuildEnvironment::simple(host_platform, host_virtual_packages);
    let result = command_dispatcher
        .install_pixi_environment(InstallPixiEnvironmentSpec {
            name,
            records: pixi_records,
            prefix: rattler_conda_types::prefix::Prefix::create(prefix.root()).into_diagnostic()?,
            installed: None,
            force_reinstall: reinstall_packages.unwrap_or_default(),
            ignore_packages,
            build_environment,
            channels,
            channel_config,
            variants: Some(variant_config),

            enabled_protocols: Default::default(),
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
    Ok(PythonStatus::from_transaction(&result.transaction))
}
