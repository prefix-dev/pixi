use std::collections::BTreeMap;

use miette::IntoDiagnostic;
use pixi_build_types::PlatformAndVirtualPackages;
use rattler_build::{
    NormalizedKey, metadata::PlatformWithVirtualPackages, recipe::variable::Variable,
    types::Directories,
};
use rattler_conda_types::ChannelUrl;
use rattler_virtual_packages::VirtualPackageOverrides;
use url::Url;

/// Returns the [`BuildConfigurationParams`] that will be used to construct a BuildConfiguration
pub fn build_configuration(
    channels: Vec<Url>,
    build_platform: Option<PlatformAndVirtualPackages>,
    host_platform: Option<PlatformAndVirtualPackages>,
    variant: BTreeMap<NormalizedKey, Variable>,
    directories: Directories,
) -> miette::Result<BuildConfigurationParams> {
    let build_platform = build_platform.map(|p| PlatformWithVirtualPackages {
        platform: p.platform,
        virtual_packages: p.virtual_packages.unwrap_or_default(),
    });

    let host_platform = host_platform.map(|p| PlatformWithVirtualPackages {
        platform: p.platform,
        virtual_packages: p.virtual_packages.unwrap_or_default(),
    });

    let (build_platform, host_platform) = match (build_platform, host_platform) {
        (Some(build_platform), Some(host_platform)) => (build_platform, host_platform),
        (build_platform, host_platform) => {
            let current_platform = rattler_build::metadata::PlatformWithVirtualPackages::detect(
                &VirtualPackageOverrides::from_env(),
            )
            .into_diagnostic()?;
            (
                build_platform.unwrap_or_else(|| current_platform.clone()),
                host_platform.unwrap_or(current_platform),
            )
        }
    };

    let channels = channels.into_iter().map(Into::into).collect();

    let params = BuildConfigurationParams {
        channels,
        build_platform,
        host_platform,
        variant,
        directories,
    };

    Ok(params)
}

/// The parameters used to construct a BuildConfiguration
#[derive(Debug)]
pub struct BuildConfigurationParams {
    pub channels: Vec<ChannelUrl>,
    pub build_platform: PlatformWithVirtualPackages,
    pub host_platform: PlatformWithVirtualPackages,
    pub variant: BTreeMap<NormalizedKey, Variable>,
    pub directories: Directories,
}
