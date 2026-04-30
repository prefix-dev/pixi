use std::{borrow::Borrow, sync::Arc};

use miette::Diagnostic;
use pixi_spec::{BinarySpec, SpecConversionError};
use pixi_spec_containers::DependencyMap;
use rattler_conda_types::{ChannelConfig, ChannelUrl, ParseChannelError};
use thiserror::Error;

use crate::{Cycle, SourceMetadataError, solve_conda::SolveCondaEnvironmentError};

/// Returns an error if any binary spec requests a channel that is not
/// present in the environment's channel list.
pub(crate) fn check_missing_channels(
    binary_specs: DependencyMap<rattler_conda_types::PackageName, BinarySpec>,
    channels: &[ChannelUrl],
    channel_config: &ChannelConfig,
) -> Result<(), Box<SolvePixiEnvironmentError>> {
    for (pkg, spec) in binary_specs.iter_specs() {
        if let BinarySpec::DetailedVersion(v) = spec
            && let Some(channel) = &v.channel
        {
            let base_url = channel
                .clone()
                .into_base_url(channel_config)
                .map_err(|err| {
                    Box::new(SolvePixiEnvironmentError::ParseChannelError(Arc::new(err)))
                })?;

            if !channels.iter().any(|c| c == &base_url) {
                return Err(Box::new(SolvePixiEnvironmentError::MissingChannel(
                    MissingChannelError {
                        package: pkg.as_normalized().to_string(),
                        channel: base_url,
                        advice: None,
                    },
                )));
            }
        }
    }
    Ok(())
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
    SourceCheckoutError(crate::SourceCheckoutError),

    /// Used by the compute-engine solve path where
    /// [`SourceMetadata`](crate::SourceMetadata) errors surface
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
            SourceMetadataError::SourceRecord(crate::SourceRecordError::Cycle(cycle)) => {
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
