//! Errors of `pixi audit` that are not reported by the engine or Pixi core.

use std::path::PathBuf;

use pixi_audit::LockFileAuditError;

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
pub(super) enum AuditCommandError {
    #[error("invalid {variable} value '{value}'")]
    InvalidUrl {
        variable: &'static str,
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error("failed to construct the HTTP client")]
    HttpClient(#[source] reqwest::Error),

    #[error("failed to load the authentication storage")]
    AuthStorage(#[source] rattler_networking::AuthenticationStorageError),

    #[error("failed to read '{}'", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to save '{}'", .0.display())]
    SaveManifest(PathBuf, #[source] std::io::Error),

    #[error(transparent)]
    Audit(#[from] LockFileAuditError),
}
