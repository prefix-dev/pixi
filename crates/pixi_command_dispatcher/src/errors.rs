use std::{borrow::Borrow, collections::BTreeMap, path::PathBuf, sync::Arc};

use miette::Diagnostic;
use pixi_record::VariantValue;
use pixi_spec::{SourceLocationSpec, SpecConversionError};
use rattler_conda_types::{
    ChannelUrl, ConvertSubdirError, InvalidPackageNameError, PackageName, ParseChannelError,
};
use rattler_repodata_gateway::RunExportExtractorError;
use thiserror::Error;

use crate::{
    BackendSourceBuildError, BuildBackendMetadataError, InstallPixiEnvironmentError,
    InstantiateBackendError, PackageNotProvidedError,
    build::{DependenciesError, pin_compatible::PinCompatibleError},
    cycle::Cycle,
    solve_conda::SolveCondaEnvironmentError,
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

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceMetadataError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceRecord(#[from] SourceRecordError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceRecordError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    BuildBackendMetadata(#[from] BuildBackendMetadataError),

    #[error("failed to amend run exports for {0} environment")]
    RunExportsExtraction(String, #[source] Arc<RunExportExtractorError>),

    #[error("while trying to solve the build environment for the package")]
    SolveBuildEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error("while trying to solve the host environment for the package")]
    SolveHostEnvironment(
        #[diagnostic_source]
        #[source]
        Box<SolvePixiEnvironmentError>,
    ),

    #[error(transparent)]
    SpecConversionError(Arc<SpecConversionError>),

    #[error(transparent)]
    InvalidPackageName(Arc<InvalidPackageNameError>),

    #[error(transparent)]
    PinCompatibleError(#[from] PinCompatibleError),

    #[error("found two source dependencies for {} but for different sources ({source1} and {source2})", package.as_source()
    )]
    DuplicateSourceDependency {
        package: PackageName,
        source1: Box<SourceLocationSpec>,
        source2: Box<SourceLocationSpec>,
    },

    #[error("the dependencies of some packages in the environment form a cycle")]
    Cycle(Cycle),

    #[error(transparent)]
    #[diagnostic(transparent)]
    PackageNotProvided(#[from] PackageNotProvidedError),

    #[error(
        "no output with matching variants found for package '{package}' at '{manifest_path}', available outputs: {available}"
    )]
    NoMatchingVariant {
        package: String,
        manifest_path: String,
        available: String,
    },

    /// Pinning or checking out the source failed.
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckout(#[from] SourceCheckoutError),
}

impl From<SpecConversionError> for SourceRecordError {
    fn from(err: SpecConversionError) -> Self {
        Self::SpecConversionError(Arc::new(err))
    }
}

impl From<InvalidPackageNameError> for SourceRecordError {
    fn from(err: InvalidPackageNameError) -> Self {
        Self::InvalidPackageName(Arc::new(err))
    }
}

impl From<DependenciesError> for SourceRecordError {
    fn from(value: DependenciesError) -> Self {
        match value {
            DependenciesError::InvalidPackageName(error) => {
                SourceRecordError::InvalidPackageName(error)
            }
            DependenciesError::PinCompatibleError(error) => {
                SourceRecordError::PinCompatibleError(error)
            }
        }
    }
}

/// An error that might be returned when solving a pixi environment.
#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SolvePixiEnvironmentError {
    #[error(transparent)]
    QueryError(Arc<rattler_repodata_gateway::GatewayError>),

    #[error("failed to solve the environment")]
    SolveError(#[source] Arc<rattler_solve::SolveError>),

    #[error(transparent)]
    SpecConversionError(Arc<SpecConversionError>),

    #[error("detected a cyclic dependency:\n\n{0}")]
    Cycle(Cycle),

    #[error(transparent)]
    ParseChannelError(Arc<ParseChannelError>),

    #[error(transparent)]
    #[diagnostic(transparent)]
    MissingChannel(MissingChannelError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    DevSourceMetadataError(crate::DevSourceMetadataError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceCheckoutError(SourceCheckoutError),

    /// [`SourceMetadata`](crate::SourceMetadata) error surfaced
    /// directly from [`SourceMetadataKey`](crate::keys::SourceMetadataKey).
    #[error(transparent)]
    #[diagnostic(transparent)]
    SourceMetadata(SourceMetadataError),
}

impl From<SourceMetadataError> for SolvePixiEnvironmentError {
    fn from(err: SourceMetadataError) -> Self {
        // Preserve cycle-error identity when the SourceMetadata error
        // ultimately wraps a SourceRecord cycle, so callers of the new
        // path still see `SolvePixiEnvironmentError::Cycle(..)` and
        // not a generic source-metadata error.
        match err {
            SourceMetadataError::SourceRecord(SourceRecordError::Cycle(cycle)) => {
                SolvePixiEnvironmentError::Cycle(cycle)
            }
            other => SolvePixiEnvironmentError::SourceMetadata(other),
        }
    }
}

impl From<rattler_repodata_gateway::GatewayError> for SolvePixiEnvironmentError {
    fn from(err: rattler_repodata_gateway::GatewayError) -> Self {
        Self::QueryError(Arc::new(err))
    }
}

impl From<rattler_solve::SolveError> for SolvePixiEnvironmentError {
    fn from(err: rattler_solve::SolveError) -> Self {
        Self::SolveError(Arc::new(err))
    }
}

impl From<SpecConversionError> for SolvePixiEnvironmentError {
    fn from(err: SpecConversionError) -> Self {
        Self::SpecConversionError(Arc::new(err))
    }
}

impl From<ParseChannelError> for SolvePixiEnvironmentError {
    fn from(err: ParseChannelError) -> Self {
        Self::ParseChannelError(Arc::new(err))
    }
}

/// An error for a missing channel in the solve request
#[derive(Debug, Clone, Diagnostic, Error)]
#[error("Package '{package}' requested unavailable channel '{channel}'")]
pub struct MissingChannelError {
    pub package: String,
    pub channel: ChannelUrl,
    #[help]
    pub advice: Option<String>,
}

impl Borrow<dyn Diagnostic> for Box<SolvePixiEnvironmentError> {
    fn borrow(&self) -> &(dyn Diagnostic + 'static) {
        self.as_ref()
    }
}

impl From<SolveCondaEnvironmentError> for SolvePixiEnvironmentError {
    fn from(err: SolveCondaEnvironmentError) -> Self {
        match err {
            SolveCondaEnvironmentError::SolveError(err) => {
                SolvePixiEnvironmentError::SolveError(Arc::new(err))
            }
            SolveCondaEnvironmentError::SpecConversionError(err) => {
                SolvePixiEnvironmentError::SpecConversionError(Arc::new(err))
            }
            SolveCondaEnvironmentError::Gateway(err) => {
                SolvePixiEnvironmentError::QueryError(Arc::new(err))
            }
        }
    }
}

impl From<crate::DevSourceMetadataError> for SolvePixiEnvironmentError {
    fn from(err: crate::DevSourceMetadataError) -> Self {
        Self::DevSourceMetadataError(err)
    }
}
