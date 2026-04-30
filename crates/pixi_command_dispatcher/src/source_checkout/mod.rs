mod git;
mod url;

pub use git::{GitCheckoutSemaphore, GitSourceCheckoutExt, HasGitCheckoutSemaphore};
pub use url::{HasUrlCheckoutSemaphore, UrlCheckoutSemaphore, UrlSourceCheckoutExt};

use crate::path::RootDirExt;
use futures::FutureExt;
use futures::future::BoxFuture;
use miette::Diagnostic;
use pixi_compute_engine::ComputeCtx;
use pixi_git::{GitError, source::Fetch as GitFetch};
use pixi_path::{AbsPathBuf, NormalizeError};
use pixi_record::{PinnedGitSpec, PinnedPathSpec, PinnedSourceSpec};
use pixi_spec::{SourceLocationSpec, UrlSpec};
use pixi_url::UrlError;
use std::path::PathBuf;
use thiserror::Error;

/// Location of the source code for a package. This will be used as the input
/// for the build process. Archives are unpacked, git clones are checked out,
/// etc.
#[derive(Debug, Clone, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SourceCheckout {
    /// The path to where the source is located locally on disk.
    pub path: AbsPathBuf,

    /// The exact source specification
    pub pinned: PinnedSourceSpec,
}

impl SourceCheckout {
    /// Returns true if the contents of the source checkout are immutable.
    pub fn is_immutable(&self) -> bool {
        self.pinned.is_immutable()
    }

    /// Construct an instance for a git spec.
    pub fn from_git(fetch: GitFetch, pinned: PinnedGitSpec) -> Self {
        let root_dir = AbsPathBuf::new(fetch.into_path())
            .expect("git checkout returned a relative path")
            .into_assume_dir();

        let path = if !pinned.source.subdirectory.is_empty() {
            root_dir
                .join(pinned.source.subdirectory.as_path())
                .into_assume_dir()
        } else {
            root_dir
        };

        Self {
            path: path.into(),
            pinned: PinnedSourceSpec::Git(pinned),
        }
    }
}

#[derive(Debug, Clone, Error, Diagnostic)]
pub enum SourceCheckoutError {
    #[error(transparent)]
    InvalidPath(#[from] InvalidPathError),

    #[error(transparent)]
    GitError(#[from] GitError),

    #[error(transparent)]
    UrlError(#[from] UrlError),

    #[error("the manifest path {0} should have a parent directory")]
    ParentDir(PathBuf),

    #[error("the subdirectory {0} does not exist in the source checkout")]
    SubdirDoesNotExist(PathBuf),

    #[error("the subdirectory {0} refers to a file in the source checkout")]
    SubdirIsAFile(PathBuf),
}

#[derive(Debug, Clone, Error)]
pub enum InvalidPathError {
    #[error("the path escapes the root directory: {0}")]
    RelativePathEscapesRoot(PathBuf),

    #[error("could not determine the current home directory while resolving: {0}")]
    CouldNotDetermineHomeDirectory(PathBuf),
}

impl From<NormalizeError> for InvalidPathError {
    fn from(value: NormalizeError) -> Self {
        match value {
            NormalizeError::EscapesRoot(path) => InvalidPathError::RelativePathEscapesRoot(path),
        }
    }
}

/// A trait to simplify async checking-out sources from different types of
/// source specifications.
pub trait SourceCheckoutExt {
    /// Checks out a particular source based on a source location spec.
    ///
    /// This function resolves the source specification to a concrete checkout
    /// by:
    /// 1. For path sources: Resolving relative paths against the root directory or against an alternative root path
    ///
    /// i.e. in the case of an out-of-tree build.
    /// Some examples for different inputs:
    /// - `/foo/bar` => `/foo/bar` (absolute paths are unchanged)
    /// - `./bar` => `<root_dir>/bar`
    /// - `bar` => `<root_dir>/bar` (or `<alternative_root>/bar` if provided)
    /// - `../bar` => `<alternative_root>/../bar` (normalized, validated for security)
    /// - `~/bar` => `<home_dir>/bar`
    ///
    /// Usually:
    /// * `root_dir` => workspace root directory (parent of workspace manifest)
    /// * `alternative_root` => package root directory (parent of package manifest)
    ///
    /// 2. For git sources: Cloning or fetching the repository and checking out
    ///    the specified reference
    /// 3. For URL sources: Downloading and extracting the archive
    ///
    /// The function handles path normalization and ensures security by
    /// preventing directory traversal attacks. It also manages caching of
    /// source checkouts to avoid redundant downloads or clones when the
    /// same source is used multiple times.
    fn pin_and_checkout(
        &mut self,
        source_location_spec: SourceLocationSpec,
    ) -> impl std::future::Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;

    /// Checkout pinned source record.
    ///
    /// Similar to `pin_and_checkout` but works with already pinned source
    /// specifications. This is used when we have a concrete revision (e.g.,
    /// a specific git commit) that we want to check out rather than
    /// resolving a reference like a branch name.
    ///
    /// The method handles different source types appropriately:
    /// - For path sources: Resolves and validates the path
    /// - For git sources: Checks out the specific revision
    /// - For URL sources: Extracts the archive with the exact checksum
    fn checkout_pinned_source(
        &mut self,
        pinned_spec: PinnedSourceSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<Self>;
}
impl SourceCheckoutExt for ComputeCtx {
    fn pin_and_checkout(
        &mut self,
        source_location_spec: SourceLocationSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let fut: BoxFuture<'static, Result<SourceCheckout, SourceCheckoutError>> =
            match source_location_spec {
                SourceLocationSpec::Url(url) => self
                    .pin_and_checkout_url(UrlSpec {
                        url: url.url,
                        md5: url.md5,
                        sha256: url.sha256,
                        subdirectory: url.subdirectory,
                    })
                    .boxed(),
                SourceLocationSpec::Path(path) => {
                    let result = self.resolve_typed_path(path.path.to_path());
                    async move {
                        Ok(SourceCheckout {
                            path: result?,
                            pinned: PinnedSourceSpec::Path(PinnedPathSpec { path: path.path }),
                        })
                    }
                    .boxed()
                }
                SourceLocationSpec::Git(git_spec) => self.pin_and_checkout_git(git_spec).boxed(),
            };
        fut
    }

    fn checkout_pinned_source(
        &mut self,
        pinned_spec: PinnedSourceSpec,
    ) -> impl Future<Output = Result<SourceCheckout, SourceCheckoutError>> + Send + use<> {
        let fut: BoxFuture<'static, Result<SourceCheckout, SourceCheckoutError>> = match pinned_spec
        {
            PinnedSourceSpec::Path(path_spec) => {
                let path_result = self.resolve_typed_path(path_spec.path.to_path());
                async move {
                    Ok(SourceCheckout {
                        path: path_result?,
                        pinned: PinnedSourceSpec::Path(path_spec),
                    })
                }
                .boxed()
            }
            PinnedSourceSpec::Git(git_spec) => self.checkout_pinned_git(git_spec).boxed(),
            PinnedSourceSpec::Url(url_spec) => self.checkout_pinned_url(url_spec).boxed(),
        };
        fut
    }
}
