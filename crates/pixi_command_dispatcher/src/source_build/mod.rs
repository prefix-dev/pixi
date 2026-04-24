//! Error type for source builds driven through
//! [`SourceBuildKey`](crate::keys::SourceBuildKey).

use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

use miette::Diagnostic;
use pixi_record::VariantValue;
use rattler_conda_types::{ConvertSubdirError, InvalidPackageNameError};
use thiserror::Error;

use crate::{
    BackendSourceBuildError, InstallPixiEnvironmentError, InstantiateBackendError,
    build::{DependenciesError, pin_compatible::PinCompatibleError},
    source_checkout::SourceCheckoutError,
};

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceBuildError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),

    #[error(transparent)]
    CreateWorkDirectory(Arc<std::io::Error>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Discovery(Arc<pixi_build_discovery::DiscoveryError>),

    #[error("could not initialize the build-backend")]
    Initialize(
        #[diagnostic_source]
        #[source]
        InstantiateBackendError,
    ),

    #[error("failed to create the build environment directory")]
    CreateBuildEnvironmentDirectory(#[source] Arc<std::io::Error>),

    #[error("failed to install the build environment")]
    InstallBuildEnvironment(#[source] Arc<InstallPixiEnvironmentError>),

    #[error("failed to install the host environment")]
    InstallHostEnvironment(#[source] Arc<InstallPixiEnvironmentError>),

    #[error(
        "The build backend does not provide an output matching '{name}' with variants {variants:?}."
    )]
    MissingOutput {
        name: String,
        variants: BTreeMap<String, VariantValue>,
    },

    #[error(
        "The build backend returned a path for the build package ({0}), but the path does not exist."
    )]
    MissingOutputFile(PathBuf),

    #[error("backend returned a dependency on an invalid package name")]
    InvalidPackageName(#[source] Arc<InvalidPackageNameError>),

    #[error(transparent)]
    PinCompatibleError(#[from] PinCompatibleError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    BackendBuildError(#[from] BackendSourceBuildError),

    #[error("failed to read metadata from the output package")]
    ReadIndexJson(#[source] Arc<rattler_package_streaming::ExtractError>),

    #[error("failed to calculate sha256 hash of {}", .0.display())]
    CalculateSha256(std::path::PathBuf, #[source] Arc<std::io::Error>),

    #[error("the package does not contain a valid subdir")]
    ConvertSubdir(#[source] Arc<ConvertSubdirError>),

    #[error(transparent)]
    GlobSet(Arc<pixi_glob::GlobSetError>),
}

impl From<InvalidPackageNameError> for SourceBuildError {
    fn from(err: InvalidPackageNameError) -> Self {
        Self::InvalidPackageName(Arc::new(err))
    }
}

impl From<pixi_glob::GlobSetError> for SourceBuildError {
    fn from(err: pixi_glob::GlobSetError) -> Self {
        Self::GlobSet(Arc::new(err))
    }
}

impl From<pixi_build_discovery::DiscoveryError> for SourceBuildError {
    fn from(err: pixi_build_discovery::DiscoveryError) -> Self {
        Self::Discovery(Arc::new(err))
    }
}

impl From<DependenciesError> for SourceBuildError {
    fn from(value: DependenciesError) -> Self {
        match value {
            DependenciesError::InvalidPackageName(error) => {
                SourceBuildError::InvalidPackageName(error)
            }
            DependenciesError::PinCompatibleError(error) => {
                SourceBuildError::PinCompatibleError(error)
            }
        }
    }
}
