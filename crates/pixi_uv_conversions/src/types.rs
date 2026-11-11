use std::error::Error;
use std::fmt::{Debug, Display};
use thiserror::Error;
use uv_pep440::VersionSpecifierBuildError;

#[derive(Debug)]
pub enum NameError {
    PepNameError(pep508_rs::InvalidNameError),
    PepExtraNameError(pep508_rs::InvalidNameError),
    UvNameError(uv_normalize::InvalidNameError),
    UvExtraNameError(uv_normalize::InvalidNameError),
}

impl Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameError::PepNameError(e) => write!(f, "Failed to convert to pep name {e}"),
            NameError::UvNameError(e) => write!(f, "Failed to convert to uv name  {e}"),
            NameError::PepExtraNameError(e) => {
                write!(f, "Failed to convert to uv extra name  {e}")
            }
            NameError::UvExtraNameError(e) => {
                write!(f, "Failed to convert to uv extra name  {e}")
            }
        }
    }
}

impl Error for NameError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            NameError::PepNameError(e) => Some(e),
            NameError::UvNameError(e) => Some(e),
            NameError::PepExtraNameError(e) => Some(e),
            NameError::UvExtraNameError(e) => Some(e),
        }
    }
}

#[derive(Debug)]
pub enum VersionSpecifiersError {
    PepVersionError(pep440_rs::VersionSpecifiersParseError),
    UvVersionError(uv_pep440::VersionSpecifiersParseError),
}

impl Display for VersionSpecifiersError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionSpecifiersError::PepVersionError(e) => {
                write!(f, "Failed to convert to pep version {e}")
            }
            VersionSpecifiersError::UvVersionError(e) => {
                write!(f, "Failed to convert to uv version  {e}")
            }
        }
    }
}

impl Error for VersionSpecifiersError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            VersionSpecifiersError::PepVersionError(e) => Some(e),
            VersionSpecifiersError::UvVersionError(e) => Some(e),
        }
    }
}

#[derive(Debug)]
pub enum Pep508Error {
    Pep508Error(pep508_rs::Pep508Error),
    UvPep508(uv_pep508::Pep508Error),
}

impl Display for Pep508Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pep508Error::Pep508Error(e) => write!(f, "Failed to convert {e}"),
            Pep508Error::UvPep508(e) => write!(f, "Failed to convert to convert {e}"),
        }
    }
}

impl Error for Pep508Error {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Pep508Error::Pep508Error(e) => Some(e),
            Pep508Error::UvPep508(e) => Some(e),
        }
    }
}

#[derive(Debug)]
pub enum VersionError {
    PepError(pep440_rs::VersionParseError),
    UvError(uv_pep440::VersionParseError),
}

impl Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VersionError::PepError(e) => write!(f, "Failed to convert {e}"),
            VersionError::UvError(e) => write!(f, "Failed to convert to convert {e}"),
        }
    }
}

impl Error for VersionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            VersionError::PepError(e) => Some(e),
            VersionError::UvError(e) => Some(e),
        }
    }
}

/// List of errors that can occur during conversion from/to uv and pep508
#[derive(Error, Debug)]
pub enum ConversionError {
    #[error(transparent)]
    InvalidPackageName(#[from] NameError),

    #[error(transparent)]
    Pep508Error(#[from] Pep508Error),

    #[error("Invalid marker environment serialization")]
    MarkerEnvironmentSerialization(#[from] serde_json::Error),

    #[error(transparent)]
    InvalidVersionSpecifier(#[from] VersionSpecifiersError),

    #[error(transparent)]
    InvalidVersionSpecifierBuild(#[from] VersionSpecifierBuildError),

    #[error(transparent)]
    InvalidVersion(#[from] VersionError),

    #[error(transparent)]
    TrustedHostError(#[from] uv_configuration::TrustedHostError),

    #[error(transparent)]
    FmtError(#[from] std::fmt::Error),
}
