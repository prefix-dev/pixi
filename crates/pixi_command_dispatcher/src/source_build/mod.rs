use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    path::PathBuf,
    str::FromStr,
};

use futures::{SinkExt, channel::mpsc::UnboundedSender};
use itertools::Itertools;
use miette::Diagnostic;
use pixi_build_discovery::{DiscoveredBackend, EnabledProtocols};
use pixi_build_frontend::json_rpc::CommunicationError;
use pixi_build_types::{
    ChannelConfiguration, PlatformAndVirtualPackages,
    procedures::conda_build::{CondaBuildParams, CondaOutputIdentifier},
};
use pixi_record::SourceRecord;
use rattler_conda_types::{ChannelConfig, ChannelUrl, Platform, Version};
use thiserror::Error;
use tracing::instrument;

use crate::{
    CommandDispatcher, CommandDispatcherError, CommandDispatcherErrorResultExt,
    InstantiateBackendError, InstantiateBackendSpec, SourceCheckout, SourceCheckoutError,
    build::WorkDirKey,
};

/// Describes all parameters required to build a conda package from a pixi
/// source package.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SourceBuildSpec {
    /// The source specification
    pub source: SourceRecord,

    /// The channel configuration to use when resolving metadata
    pub channel_config: ChannelConfig,

    /// The channels to use for solving.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<ChannelUrl>,

    /// Information about host platform on which the package is build. Note that
    /// a package might be targeting noarch in which case the host platform
    /// should be used.
    ///
    /// If this field is omitted the build backend will use the current
    /// platform.
    pub host_platform: Option<PlatformAndVirtualPackages>,

    /// Variant configuration
    pub variants: Option<BTreeMap<String, Vec<String>>>,

    /// The protocols that are enabled for this source
    #[serde(skip_serializing_if = "crate::is_default")]
    pub enabled_protocols: EnabledProtocols,
}

pub struct BuiltSource {
    /// The source checkout that was built
    pub source: SourceCheckout,

    /// The location on disk where the built package is located.
    pub output_file: PathBuf,

    /// The globs that were used as input to the build. Use these for
    /// re-verifying the build.
    pub input_globs: BTreeSet<String>,
}

impl SourceBuildSpec {
    #[instrument(skip_all, fields(
        source = %self.source.source,
        subdir = %self.source.package_record.subdir,
        name = %self.source.package_record.name.as_normalized(),
        version = %self.source.package_record.version,
        build = %self.source.package_record.build))]
    pub(crate) async fn build(
        self,
        command_dispatcher: CommandDispatcher,
        mut log_sink: UnboundedSender<String>,
    ) -> Result<BuiltSource, CommandDispatcherError<SourceBuildError>> {
        tracing::debug!("Building package for source spec: {}", self.source.source);

        // Check out the source code.
        let source_checkout = command_dispatcher
            .checkout_pinned_source(self.source.source.clone())
            .await
            .map_err_with(SourceBuildError::SourceCheckout)?;

        // Discover information about the build backend from the source code.
        let discovered_backend = DiscoveredBackend::discover(
            &source_checkout.path,
            &self.channel_config,
            &self.enabled_protocols,
        )
        .map_err(SourceBuildError::Discovery)
        .map_err(CommandDispatcherError::Failed)?;

        // Instantiate the backend with the discovered information.
        let backend = command_dispatcher
            .instantiate_backend(InstantiateBackendSpec {
                backend_spec: discovered_backend.backend_spec,
                init_params: discovered_backend.init_params,
                channel_config: self.channel_config.clone(),
                enabled_protocols: self.enabled_protocols,
            })
            .await
            .map_err_with(SourceBuildError::Initialize)?;

        // Use the backend to build the source package.
        let mut build_result = backend
            .conda_build(
                CondaBuildParams {
                    build_platform_virtual_packages: Some(
                        command_dispatcher.tool_platform().1.to_vec(),
                    ),
                    channel_base_urls: Some(self.channels.into_iter().map(Into::into).collect()),
                    channel_configuration: ChannelConfiguration {
                        base_url: self.channel_config.channel_alias.clone(),
                    },
                    outputs: Some(BTreeSet::from_iter([CondaOutputIdentifier {
                        name: Some(self.source.package_record.name.as_normalized().to_string()),
                        version: Some(self.source.package_record.version.to_string()),
                        build: Some(self.source.package_record.build.clone()),
                        subdir: Some(self.source.package_record.subdir.clone()),
                    }])),
                    variant_configuration: self
                        .variants
                        .map(|variants| variants.into_iter().collect()),
                    work_directory: command_dispatcher.cache_dirs().working_dirs().join(
                        WorkDirKey {
                            source: Box::new(self.source.clone()).into(),
                            host_platform: self
                                .host_platform
                                .as_ref()
                                .map(|platform| platform.platform)
                                .unwrap_or(Platform::current()),
                            build_backend: backend.identifier().to_string(),
                        }
                        .key(),
                    ),
                    host_platform: self.host_platform,
                    editable: !self.source.source.is_immutable(),
                },
                move |line| {
                    let _err = futures::executor::block_on(log_sink.send(line));
                },
            )
            .await
            .map_err(SourceBuildError::BuildError)
            .map_err(CommandDispatcherError::Failed)?;

        // If the backend returned more packages than expected output a warning.
        if build_result.packages.len() > 1 {
            let pkgs = build_result.packages.iter().format_with(", ", |pkg, f| {
                f(&format_args!(
                    "{}/{}={}={}",
                    pkg.subdir, pkg.name, pkg.version, pkg.build,
                ))
            });
            tracing::warn!(
                "While building {} for {}, the build backend returned more packages than expected: {pkgs}. Only the package matching the source record will be used.",
                self.source.source,
                self.source.package_record.subdir,
            );
        }

        // Locate the package that matches the source record we requested to be build.
        let built_package = if let Some(idx) = build_result.packages.iter().position(|pkg| {
            pkg.name == self.source.package_record.name.as_normalized()
                && Version::from_str(&pkg.version).ok().as_ref()
                    == Some(&self.source.package_record.version)
                && pkg.build == self.source.package_record.build
                && pkg.subdir == self.source.package_record.subdir
        }) {
            build_result.packages.swap_remove(idx)
        } else {
            return Err(CommandDispatcherError::Failed(
                UnexpectedPackageError {
                    subdir: self.source.package_record.subdir.clone(),
                    name: self.source.package_record.name.as_normalized().to_string(),
                    version: self.source.package_record.version.to_string(),
                    build: self.source.package_record.build.clone(),
                    packages: build_result
                        .packages
                        .iter()
                        .map(|pkg| {
                            format!("{}/{}={}={}", pkg.subdir, pkg.name, pkg.version, pkg.build)
                        })
                        .collect(),
                }
                .into(),
            ));
        };

        Ok(BuiltSource {
            source: source_checkout,
            input_globs: built_package.input_globs,
            output_file: built_package.output_file,
        })
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum SourceBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(#[from] pixi_build_discovery::DiscoveryError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Initialize(#[from] InstantiateBackendError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildError(#[from] CommunicationError),

    #[error(transparent)]
    UnexpectedPackage(#[from] UnexpectedPackageError),
}

/// An error that can occur when the build backend did not return the expected
/// package.
#[derive(Debug, Error)]
pub struct UnexpectedPackageError {
    pub subdir: String,
    pub name: String,
    pub version: String,
    pub build: String,
    pub packages: Vec<String>,
}

impl Display for UnexpectedPackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.packages.len() {
            0 => write!(
                f,
                "The build backend did not return any packages for {}/{}={}={}.",
                self.subdir, self.name, self.version, self.build
            ),
            1 => write!(
                f,
                "The build backend did not return the expected package: {}/{}={}={}. Instead the build backend returned {}.",
                self.subdir, self.name, self.version, self.build, self.packages[0]
            ),
            _ => write!(
                f,
                "The build backend did not return the expected package: {}/{}={}={}. Instead the following packages were returned:\n- {}",
                self.subdir,
                self.name,
                self.version,
                self.build,
                self.packages.iter().format("\n- ")
            ),
        }
    }
}
