mod ext;
pub(crate) mod fetch_progress;
pub(crate) mod reporter;

pub use ext::InstallPixiEnvironmentExt;
pub use fetch_progress::FetchAttempt;

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    error::Error as StdError,
    ffi::OsStr,
    fmt,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use human_bytes::human_bytes;
use miette::Diagnostic;
use url::Url;

use pixi_record::{UnresolvedPixiRecord, VariantValue};
use pixi_spec::ResolvedExcludeNewer;
use pixi_utils::EnvironmentFingerprint;
use rattler::install::{
    InstallationResultRecord, InstallerError, Transaction,
    link_script::{LinkScriptError, PrePostLinkResult},
};
use rattler_conda_types::{ChannelUrl, PackageName, PrefixRecord, RepoDataRecord, prefix::Prefix};
use thiserror::Error;

use crate::{BuildEnvironment, SourceBuildError};
use fetch_progress::redacted;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct InstallPixiEnvironmentSpec {
    pub name: String,

    /// Records to install; partial source records are built from source.
    #[serde(skip)]
    pub records: Vec<UnresolvedPixiRecord>,

    /// Packages neither removed when missing from `records` nor updated
    /// when already installed.
    pub ignore_packages: Option<HashSet<PackageName>>,

    #[serde(skip)]
    pub prefix: Prefix,

    #[serde(skip)]
    pub installed: Option<Vec<PrefixRecord>>,

    pub build_environment: BuildEnvironment,

    #[serde(skip_serializing_if = "HashSet::is_empty")]
    pub force_reinstall: HashSet<rattler_conda_types::PackageName>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub exclude_newer: Option<ResolvedExcludeNewer>,

    pub channels: Vec<ChannelUrl>,

    pub variant_configuration: Option<BTreeMap<String, Vec<VariantValue>>>,

    pub variant_files: Option<Vec<PathBuf>>,

    /// Inline package definitions keyed by package name. Source
    /// records whose name matches build from the inline manifest instead of
    /// discovering one on disk. Empty when no inline definitions are in scope.
    #[serde(skip)]
    pub inline_packages: HashMap<PackageName, crate::InlinePackage>,
}

pub struct InstallPixiEnvironmentResult {
    pub transaction: Transaction<InstallationResultRecord, RepoDataRecord>,

    /// `None` when link scripts were disabled.
    pub pre_link_script_result: Option<PrePostLinkResult>,

    /// `None` when link scripts were disabled.
    pub post_link_script_result: Option<Result<PrePostLinkResult, LinkScriptError>>,

    /// Built repodata records for source records present in the input.
    pub resolved_source_records: HashMap<PackageName, Arc<RepoDataRecord>>,

    /// Content fingerprint of every record that landed in the prefix.
    pub installed_fingerprint: EnvironmentFingerprint,
}

impl InstallPixiEnvironmentSpec {
    pub fn new(
        records: impl IntoIterator<Item = impl Into<UnresolvedPixiRecord>>,
        prefix: Prefix,
    ) -> Self {
        let records = records.into_iter().map(Into::into).collect();
        InstallPixiEnvironmentSpec {
            name: prefix
                .file_name()
                .map(OsStr::to_string_lossy)
                .map(Cow::into_owned)
                .unwrap_or_default(),
            records,
            prefix,
            installed: None,
            ignore_packages: None,
            build_environment: BuildEnvironment::default(),
            force_reinstall: HashSet::new(),
            exclude_newer: None,
            channels: Vec::new(),
            variant_configuration: None,
            variant_files: None,
            inline_packages: HashMap::new(),
        }
    }
}

#[derive(Debug, Error, Diagnostic)]
pub enum InstallPixiEnvironmentError {
    #[error("failed to collect prefix records from '{}'", .0.path().display())]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ReadInstalledPackages(Prefix, #[source] std::io::Error),

    /// A package download failed. Split out from [`Self::Installer`] so the
    /// request context the installer never puts in `InstallerError` — the URL,
    /// how much of the transfer completed, and how long it ran — is reported
    /// alongside the cause chain.
    #[error("failed to fetch {package} from {url} ({progress})")]
    #[diagnostic(help("{}", fetch_help(.progress)))]
    FailedToFetch {
        /// The package archive identifier, e.g. `tzdata-2025c-hc9c84f9_1.conda`.
        package: String,
        /// Source URL with secrets redacted.
        url: Url,
        progress: FetchProgressSummary,
        #[source]
        source: Box<InstallerError>,
    },

    #[error(transparent)]
    Installer(InstallerError),

    #[error("failed to build '{}' from '{}'",
        .0.as_source(),
        .1)]
    BuildUnresolvedSourceError(
        PackageName,
        Box<pixi_record::PinnedSourceSpec>,
        #[diagnostic_source]
        #[source]
        SourceBuildError,
        #[help] Option<String>,
    ),

    #[error("failed to clear source-build cache for '{}'", .0.as_source())]
    ClearSourceBuildCache(PackageName, #[source] std::io::Error),

    #[error(
        "failed to convert install transaction to prefix records from '{}'",
        .0.path().display()
    )]
    #[diagnostic(help("try `pixi clean` to reset the environment and run the command again"))]
    ConvertTransactionToPrefixRecord(Prefix, #[source] std::io::Error),

    #[error("failed to determine python info for the installed environment: {0}")]
    DetectPythonInfo(String),

    #[error("failed to acquire install lock on prefix '{}'", .0.path().display())]
    AcquireLock(Prefix, #[source] std::io::Error),
}

#[derive(Debug, Error)]
#[error("{message}")]
struct SanitizedHttpError {
    message: String,
}

impl SanitizedHttpError {
    fn new(
        status: Option<reqwest::StatusCode>,
        is_timeout: bool,
        is_body: bool,
        is_decode: bool,
        is_request: bool,
        url: Option<&Url>,
    ) -> Self {
        let description = match status {
            Some(status) if status.is_client_error() => {
                format!("HTTP status client error ({status})")
            }
            Some(status) if status.is_server_error() => {
                format!("HTTP status server error ({status})")
            }
            Some(status) => format!("HTTP status error ({status})"),
            None if is_timeout => "HTTP request timed out".to_string(),
            None if is_body => "HTTP response body error".to_string(),
            None if is_decode => "HTTP response decode error".to_string(),
            None if is_request => "HTTP request failed".to_string(),
            None => "HTTP interaction failed".to_string(),
        };
        let message = url.map_or(description.clone(), |url| {
            format!("{description} for url ({})", redacted(url))
        });
        Self { message }
    }

    fn from_middleware(error: &reqwest_middleware::Error) -> Self {
        Self::new(
            error.status(),
            error.is_timeout(),
            error.is_body(),
            error.is_decode(),
            error.is_request(),
            error.url(),
        )
    }

    fn from_reqwest(error: &reqwest::Error) -> Self {
        Self::new(
            error.status(),
            error.is_timeout(),
            error.is_body(),
            error.is_decode(),
            error.is_request(),
            error.url(),
        )
    }
}

fn sanitized_http_error(mut error: &(dyn StdError + 'static)) -> Option<SanitizedHttpError> {
    loop {
        if let Some(rattler::package_cache::PackageCacheLayerError::FetchError(source)) =
            error.downcast_ref::<rattler::package_cache::PackageCacheLayerError>()
            && let Some(error) = sanitized_http_error(source.as_ref())
        {
            return Some(error);
        }
        if let Some(rattler_package_streaming::ExtractError::ReqwestError(error)) =
            error.downcast_ref::<rattler_package_streaming::ExtractError>()
        {
            return Some(SanitizedHttpError::from_middleware(error));
        }
        if let Some(error) = error.downcast_ref::<reqwest_middleware::Error>() {
            return Some(SanitizedHttpError::from_middleware(error));
        }
        if let Some(error) = error.downcast_ref::<reqwest::Error>() {
            return Some(SanitizedHttpError::from_reqwest(error));
        }
        error = error.source()?;
    }
}

pub(super) fn sanitized_fetch_error(error: InstallerError) -> InstallerError {
    let sanitized = sanitized_http_error(&error);
    match (error, sanitized) {
        (InstallerError::FailedToFetch(package, _), Some(source)) => InstallerError::FailedToFetch(
            package,
            rattler::package_cache::PackageCacheError::LayerError(Box::new(source)),
        ),
        (error, _) => error,
    }
}

/// How far a failed package download got, rendered for an error message.
#[derive(Debug, Clone, Copy)]
pub struct FetchProgressSummary {
    /// Bytes received before the failure.
    pub transferred: u64,
    /// Total expected, when the server or repodata told us.
    pub expected: Option<u64>,
    /// How long the transfer ran before it was abandoned.
    pub elapsed: Duration,
}

impl From<&FetchAttempt> for FetchProgressSummary {
    fn from(attempt: &FetchAttempt) -> Self {
        Self {
            transferred: attempt.transferred,
            expected: attempt.expected,
            elapsed: attempt.elapsed,
        }
    }
}

impl FetchProgressSummary {
    /// True when the transfer started but did not finish, which is the
    /// signature of a stalled or reset connection rather than, say, a 404.
    fn stalled(&self) -> bool {
        self.expected
            .is_some_and(|expected| self.transferred > 0 && self.transferred < expected)
    }
}

impl fmt::Display for FetchProgressSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let transferred = human_bytes(self.transferred as f64);
        match self.expected {
            Some(expected) => write!(
                f,
                "{transferred} of {} transferred after {:.1?}",
                human_bytes(expected as f64),
                self.elapsed
            ),
            None => write!(f, "{transferred} transferred after {:.1?}", self.elapsed),
        }
    }
}

/// Help text for a failed fetch. A partial transfer points at the network
/// path, so suggest the knobs that affect it.
fn fetch_help(progress: &FetchProgressSummary) -> String {
    if progress.stalled() {
        "the download did not complete. Check your network connection or proxy configuration, or \
         reduce `concurrency.downloads`."
            .to_string()
    } else {
        "check that the channel is reachable and that the package still exists in it.".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler::package_cache::{PackageCacheError, PackageCacheLayerError};
    use reqwest::ResponseBuilderExt;

    #[test]
    fn failed_fetch_diagnostic_does_not_render_query_secrets() {
        let package = "tzdata-2026c-h151e31d_0.conda";
        let signed_url = Url::parse(
            "https://packages.example.test/signed/package.conda?source=source-secret&signature=signature-secret&vendor_token=unknown-secret#fragment-secret",
        )
        .unwrap();
        let response: reqwest::Response = http::Response::builder()
            .status(reqwest::StatusCode::NOT_FOUND)
            .url(signed_url)
            .body(Vec::<u8>::new())
            .unwrap()
            .into();
        let request_error = response.error_for_status().unwrap_err();
        let extract_error = rattler_package_streaming::ExtractError::ReqwestError(
            reqwest_middleware::Error::Reqwest(request_error),
        );
        let cache_error = PackageCacheError::LayerError(Box::new(
            PackageCacheLayerError::FetchError(Arc::new(extract_error)),
        ));
        let source = sanitized_fetch_error(InstallerError::FailedToFetch(
            package.to_string(),
            cache_error,
        ));
        let error = InstallPixiEnvironmentError::FailedToFetch {
            package: package.to_string(),
            url: Url::parse("https://packages.example.test/package.conda").unwrap(),
            progress: FetchProgressSummary {
                transferred: 0,
                expected: Some(1024),
                elapsed: Duration::from_secs(2),
            },
            source: Box::new(source),
        };

        let rendered = pixi_test_utils::format_diagnostic(&error);
        assert!(rendered.contains("404 Not Found"), "{rendered}");
        assert!(
            rendered.contains("https://packages.example.test/signed/package.conda"),
            "{rendered}"
        );
        assert!(!rendered.contains("package.conda?"), "{rendered}");
        for secret in [
            "source-secret",
            "signature-secret",
            "unknown-secret",
            "fragment-secret",
            "source=",
            "signature=",
            "vendor_token=",
        ] {
            assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
        }
    }
}
