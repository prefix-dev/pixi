//! Offline-mode error handling.
//!
//! Pixi enforces offline mode (`--offline`, `PIXI_OFFLINE`, or the `offline`
//! config option) in the layers where network access happens: the reqwest
//! middleware stack rejects every request, the repodata gateway only reads
//! from its cache, uv runs with `Connectivity::Offline`, and git fetches are
//! restricted to the local `file` transport. The errors those layers produce
//! are correct but terse; this module recognizes them and attaches a hint
//! explaining that offline mode caused the failure and how to get out of it.

use std::fmt::{self, Display};

use miette::{Diagnostic, LabeledSpan, Report, Severity, SourceCode};
use thiserror::Error;

/// Hint attached to errors that were caused by running in offline mode.
const OFFLINE_HINT: &str = "pixi is running in offline mode and only uses locally cached data.\nRetry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.";

/// Error returned by commands that cannot do anything useful without network
/// access (e.g. `pixi self-update`, `pixi upload`) when pixi runs in offline
/// mode.
#[derive(Debug, Error, Diagnostic)]
#[error("`{command}` requires network access, but pixi is running in offline mode")]
#[diagnostic(help(
    "retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration"
))]
pub struct NetworkRequiredError {
    /// The command that requires network access, e.g. `pixi self-update`.
    pub command: &'static str,
}

/// Messages produced by the layers that enforce offline mode. An error chain
/// containing one of these failed *because* pixi is in offline mode.
///
/// The enforcement points live in external crates (rattler, uv) and their
/// errors typically cross several `anyhow`/`miette` wrapping boundaries that
/// erase the concrete types, so the messages are matched instead of
/// downcasting. Markers must be lowercase; matching is case-insensitive.
const OFFLINE_ERROR_MARKERS: &[&str] = &[
    // `rattler_networking::OfflineMiddleware` rejecting a request.
    "network access is disabled by offline mode",
    // `rattler_repodata_gateway` cache miss with `CacheAction::ForceCacheOnly`.
    "there is no cache available",
    // The sharded-repodata variants of the same cache miss: a missing shard
    // index, and a missing per-package shard.
    "sharded index cache for",
    "the shard for package",
    // uv running with `Connectivity::Offline` ("Network connectivity is
    // disabled, but ...") and uv-git ("Remote Git fetches are not allowed
    // because network connectivity is disabled ...").
    "network connectivity is disabled",
    // `pixi_git::GitError::Offline` / `GitError::OfflineSubmodule`.
    "pixi is in offline mode",
];

fn message_matches(message: &str) -> bool {
    let message = message.to_lowercase();
    OFFLINE_ERROR_MARKERS
        .iter()
        .any(|marker| message.contains(marker))
}

/// Walk the [`Diagnostic`] tree (message + related + diagnostic_source); not
/// every diagnostic exposes its cause through `Error::source`.
fn diagnostic_tree_matches(diagnostic: &dyn Diagnostic) -> bool {
    if message_matches(&diagnostic.to_string()) {
        return true;
    }
    if let Some(related) = diagnostic.related()
        && related.into_iter().any(diagnostic_tree_matches)
    {
        return true;
    }
    diagnostic
        .diagnostic_source()
        .is_some_and(diagnostic_tree_matches)
}

/// If `report` failed because pixi runs in offline mode, wrap it so the
/// rendered diagnostic carries a hint that explains this; otherwise return it
/// unchanged.
pub fn attach_offline_hint(report: Report) -> Report {
    let chain_matches = report.chain().any(|err| message_matches(&err.to_string()));
    if chain_matches || diagnostic_tree_matches(report.as_ref()) {
        Report::new(OfflineHintWrapper { inner: report })
    } else {
        report
    }
}

/// Delegates the entire [`Diagnostic`] surface to the wrapped report and only
/// appends [`OFFLINE_HINT`] to its help text, so labels, source code, and the
/// cause chain render exactly as they would have without the wrapper.
#[derive(Debug)]
struct OfflineHintWrapper {
    inner: Report,
}

impl OfflineHintWrapper {
    fn as_diagnostic(&self) -> &(dyn Diagnostic + 'static) {
        self.inner.as_ref()
    }
}

impl Display for OfflineHintWrapper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.as_diagnostic(), f)
    }
}

impl std::error::Error for OfflineHintWrapper {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        let inner: &(dyn std::error::Error + 'static) = self.inner.as_ref();
        inner.source()
    }
}

impl Diagnostic for OfflineHintWrapper {
    fn code(&self) -> Option<Box<dyn Display + '_>> {
        self.as_diagnostic().code()
    }

    fn severity(&self) -> Option<Severity> {
        self.as_diagnostic().severity()
    }

    fn help(&self) -> Option<Box<dyn Display + '_>> {
        Some(match self.as_diagnostic().help() {
            Some(help) => Box::new(format!("{help}\n{OFFLINE_HINT}")),
            None => Box::new(OFFLINE_HINT.to_string()),
        })
    }

    fn url(&self) -> Option<Box<dyn Display + '_>> {
        self.as_diagnostic().url()
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.as_diagnostic().source_code()
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.as_diagnostic().labels()
    }

    fn related<'a>(&'a self) -> Option<Box<dyn Iterator<Item = &'a dyn Diagnostic> + 'a>> {
        self.as_diagnostic().related()
    }

    fn diagnostic_source(&self) -> Option<&dyn Diagnostic> {
        self.as_diagnostic().diagnostic_source()
    }
}

#[cfg(test)]
mod tests {
    use miette::{IntoDiagnostic, WrapErr};
    use pixi_test_utils::format_diagnostic;

    use super::*;

    /// Renders a report the way the CLI does, after passing it through
    /// [`attach_offline_hint`].
    fn render_with_hint(report: Report) -> String {
        let report = attach_offline_hint(report);
        format_diagnostic(report.as_ref())
    }

    /// A request blocked by `rattler_networking::OfflineMiddleware` gets the
    /// offline hint attached.
    #[tokio::test]
    async fn blocked_request_gets_offline_hint() {
        // Produce the real error the middleware returns instead of
        // fabricating one.
        let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new())
            .with(rattler_networking::OfflineMiddleware)
            .build();
        let err = client
            .get("https://prefix.dev/conda-forge/noarch/repodata.json")
            .send()
            .await
            .expect_err("the offline middleware rejects every request");

        let report = Err::<(), _>(err)
            .into_diagnostic()
            .wrap_err("failed to download the package")
            .unwrap_err();

        insta::assert_snapshot!(render_with_hint(report), @"
        × failed to download the package
        ╰─▶ network access is disabled by offline mode
        help: pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    }

    /// A repodata cache miss while the gateway is in cache-only mode gets the
    /// offline hint attached.
    #[test]
    fn repodata_cache_miss_gets_offline_hint() {
        let report =
            Err::<(), _>(rattler_repodata_gateway::fetch::FetchRepoDataError::NoCacheAvailable)
                .into_diagnostic()
                .wrap_err("failed to load the repodata for channel `conda-forge`")
                .unwrap_err();

        insta::assert_snapshot!(render_with_hint(report), @"
        × failed to load the repodata for channel `conda-forge`
        ╰─▶ there is no cache available
        help: pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    }

    /// A per-package shard cache miss while the sharded gateway is in
    /// cache-only mode gets the offline hint attached.
    #[test]
    fn shard_cache_miss_gets_offline_hint() {
        let report = Err::<(), _>(rattler_repodata_gateway::GatewayError::CacheError(
            "the shard for package 'libgcc' is not in the cache".to_string(),
        ))
        .into_diagnostic()
        .wrap_err("failed to solve requirements of environment 'default' for platform 'linux-64'")
        .unwrap_err();

        insta::assert_snapshot!(render_with_hint(report), @"
        × failed to solve requirements of environment 'default' for platform 'linux-64'
        ╰─▶ the shard for package 'libgcc' is not in the cache
        help: pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    }

    /// uv's `Connectivity::Offline` errors get the offline hint attached.
    #[test]
    fn uv_offline_error_gets_offline_hint() {
        // uv's error type is hard to construct directly; emulate the message
        // it produces (see `uv_client::ErrorKind::Offline`).
        let report = miette::miette!(
            "Network connectivity is disabled, but the requested data wasn't found in the cache for: `https://pypi.org/simple/httpx/`"
        );

        insta::assert_snapshot!(render_with_hint(report), @"
        × Network connectivity is disabled, but the requested data wasn't found in the cache for: `https://pypi.org/simple/httpx/`
        help: pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    }

    /// uv-git's offline error (lowercase "network connectivity is disabled")
    /// also gets the hint attached — matching is case-insensitive.
    #[test]
    fn uv_git_offline_error_gets_offline_hint() {
        // Emulates the message produced by `uv_git` (see uv-git `git.rs`,
        // `TransportNotAllowed`).
        let report = miette::miette!(
            "Remote Git fetches are not allowed because network connectivity is disabled (i.e., with `--offline`)"
        );

        insta::assert_snapshot!(render_with_hint(report), @"
        × Remote Git fetches are not allowed because network connectivity is disabled (i.e., with `--offline`)
        help: pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    }

    /// An existing help text is kept and the offline hint is appended.
    #[test]
    fn offline_hint_is_appended_to_existing_help() {
        let report = miette::miette!(
            help = "existing help text",
            "network access is disabled by offline mode"
        );

        insta::assert_snapshot!(render_with_hint(report), @"
        × network access is disabled by offline mode
        help: existing help text
              pixi is running in offline mode and only uses locally cached data.
              Retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration.
        ");
    }

    /// Unrelated errors pass through untouched.
    #[test]
    fn unrelated_errors_do_not_get_the_hint() {
        let report = miette::miette!("failed to parse the manifest");
        insta::assert_snapshot!(render_with_hint(report), @"  × failed to parse the manifest");
    }

    /// The guard error used by inherently-online commands.
    #[test]
    fn network_required_error() {
        insta::assert_snapshot!(
            format_diagnostic(&NetworkRequiredError {
                command: "pixi self-update",
            }),
            @"
        × `pixi self-update` requires network access, but pixi is running in offline mode
        help: retry with network access: remove the `--offline` flag, unset the `PIXI_OFFLINE` environment variable, or disable the `offline` option in your pixi configuration
        "
        );
    }
}
