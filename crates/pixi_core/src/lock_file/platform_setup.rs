//! Common workspace-environment setup used by both satisfiability
//! verification and the install path. Both routes need to project an
//! [`Environment`] + [`Platform`] into the same set of inputs the
//! command dispatcher expects (channels, variants, virtual packages),
//! and to allocate a [`WorkspaceEnvRef`] keyed on those inputs so all
//! downstream backend / source-record requests at this platform share
//! one workspace-env identity.
//!
//! Centralizing the construction here keeps the inputs identical
//! across call sites, which is what makes the
//! `WorkspaceEnvRegistry` dedup actually fire: drift between two
//! sites would silently mint two distinct
//! [`WorkspaceEnvRef`]s and defeat any in-memory cache the engine
//! built up under the first.

use pixi_command_dispatcher::{CommandDispatcher, EnvironmentSpec, WorkspaceEnvRef};
use pixi_compute_engine::BuildEnvironment;
use pixi_manifest::FeaturesExt;
use rattler_conda_types::{ChannelConfig, GenericVirtualPackage, ParseChannelError, Platform};
use thiserror::Error;

use crate::workspace::{Environment, HasWorkspaceRef, errors::VariantsError};

/// Resolved workspace context for a single environment + platform.
///
/// Holds the data both satisfiability verification and the install
/// path need: the resolved channel config (for downstream spec
/// conversion), the platform's virtual packages (for solves), and the
/// shared [`WorkspaceEnvRef`] that anchors source-record and
/// build-backend caches.
#[derive(Debug, Clone)]
pub(crate) struct PlatformSetup {
    pub channel_config: ChannelConfig,
    pub virtual_packages: Vec<GenericVirtualPackage>,
    pub workspace_env_ref: WorkspaceEnvRef,
}

/// Error returned by [`build_platform_setup`]. Variants mirror the
/// concrete failure modes the underlying [`Environment`] accessors
/// can produce; callers map these into their own diagnostic shapes.
#[derive(Debug, Error)]
pub(crate) enum PlatformSetupError {
    /// A workspace channel could not be normalized into a base URL.
    #[error(transparent)]
    InvalidChannel(#[from] ParseChannelError),

    /// The workspace's variant configuration for this platform is
    /// malformed.
    #[error(transparent)]
    Variants(#[from] VariantsError),
}

/// Build the [`PlatformSetup`] for `(environment, platform)` and
/// allocate a [`WorkspaceEnvRef`] from `command_dispatcher`'s
/// registry. The registry dedups equal allocations, so two callers
/// with identical inputs receive the same handle and share dispatch
/// caches.
pub(crate) fn build_platform_setup(
    environment: &Environment<'_>,
    platform: Platform,
    command_dispatcher: &CommandDispatcher,
) -> Result<PlatformSetup, PlatformSetupError> {
    let channel_config = environment.workspace().channel_config();
    let channels = environment
        .channels()
        .into_iter()
        .cloned()
        .map(|c| c.into_base_url(&channel_config))
        .collect::<Result<Vec<_>, _>>()?;
    let variant_config = environment.workspace().variants(platform)?;
    let virtual_packages: Vec<GenericVirtualPackage> = environment
        .virtual_packages(platform)
        .into_iter()
        .map(GenericVirtualPackage::from)
        .collect();
    let build_environment = BuildEnvironment {
        host_platform: platform,
        build_platform: platform,
        host_virtual_packages: virtual_packages.clone(),
        build_virtual_packages: virtual_packages.clone(),
    };

    let workspace_env_ref = command_dispatcher.workspace_env_registry().allocate(
        environment.name().as_str().to_string(),
        platform,
        EnvironmentSpec {
            channels,
            build_environment,
            variants: variant_config,
            exclude_newer: None,
            channel_priority: Default::default(),
        },
    );

    Ok(PlatformSetup {
        channel_config,
        virtual_packages,
        workspace_env_ref,
    })
}
