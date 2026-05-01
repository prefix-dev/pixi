//! [`CacheDirs`]: a typed bag of on-disk cache roots used across the
//! dispatcher (build backend envs, package cache, git/url checkouts,
//! source-build artifacts and workspaces, etc.). Each path either falls
//! back to a `<root>/<name>` default or is overridden by the caller.

use pixi_consts::consts;
use pixi_path::{AbsPresumedDirPath, AbsPresumedDirPathBuf};

use super::backend_metadata::BuildBackendMetadataCache;

#[derive(Clone)]
pub struct CacheDirs {
    /// The root cache directory, all other cache directories are derived from
    /// this.
    root: AbsPresumedDirPathBuf,

    /// Specifies the cache directory for workspace specific directories. If
    /// this is not specified the directories are stored in the global
    /// cache.
    workspace: Option<AbsPresumedDirPathBuf>,

    /// Directory where environments for build backends are cached.
    build_backends: Option<AbsPresumedDirPathBuf>,

    /// The directory where binary packages are cached.
    packages: Option<AbsPresumedDirPathBuf>,

    /// The directory where git repositories are cached.
    git: Option<AbsPresumedDirPathBuf>,

    /// The location where to store build backend metadata information.
    build_backend_metadata: Option<AbsPresumedDirPathBuf>,

    /// The directory where url archives are cached.
    url: Option<AbsPresumedDirPathBuf>,

    /// The location where to store content-addressed source-build
    /// artifacts. Defaults to `<workspace>/artifacts-v0` (or, when no
    /// workspace is configured, `<root>/artifacts-v0`).
    source_build_artifacts: Option<AbsPresumedDirPathBuf>,

    /// The location where to store per-build backend workspaces
    /// (backend incremental state). Defaults to `<workspace>/bld` so
    /// the deep nested paths every backend writes under it stay well
    /// below Windows' `MAX_PATH` limit.
    source_build_workspaces: Option<AbsPresumedDirPathBuf>,
}

impl CacheDirs {
    /// Instantiate a new `CacheDirs` instance with the given root directory.
    pub fn new(root: AbsPresumedDirPathBuf) -> Self {
        Self {
            root,
            workspace: None,
            build_backends: None,
            packages: None,
            git: None,
            build_backend_metadata: None,
            url: None,
            source_build_artifacts: None,
            source_build_workspaces: None,
        }
    }

    pub fn with_workspace(self, workspace: AbsPresumedDirPathBuf) -> Self {
        Self {
            workspace: Some(workspace),
            ..self
        }
    }

    /// Overrides the backend-metadata root. Both cached metadata
    /// entries and the backend's `conda/outputs` scratch dir nest
    /// under this path; see [`Self::backend_metadata`] for the layout.
    ///
    /// `pixi build --build-dir <path>` routes its argument here so
    /// users can aim the whole backend tree at a custom location.
    pub fn with_backend_metadata(self, dir: AbsPresumedDirPathBuf) -> Self {
        Self {
            build_backend_metadata: Some(dir),
            ..self
        }
    }

    /// See [`Self::with_backend_metadata`].
    pub fn set_backend_metadata(&mut self, dir: AbsPresumedDirPathBuf) {
        self.build_backend_metadata = Some(dir);
    }

    /// Returns the root directory for the cache.
    pub fn root(&self) -> &AbsPresumedDirPath {
        &self.root
    }

    /// Returns the workspace directory if it is set, otherwise returns `None`.
    /// For pixi this would refer to the `.pixi` directory in the workspace.
    pub fn workspace(&self) -> Option<&AbsPresumedDirPath> {
        self.workspace.as_deref()
    }

    /// Returns the directory where build backend environments are cached.
    pub fn build_backends(&self) -> AbsPresumedDirPathBuf {
        self.build_backends.clone().unwrap_or_else(|| {
            self.root
                .join(consts::CACHED_BUILD_BACKENDS)
                .into_assume_dir()
        })
    }

    /// Returns the location to store packages
    pub fn packages(&self) -> AbsPresumedDirPathBuf {
        self.packages
            .clone()
            .unwrap_or_else(|| self.root.join(consts::CACHED_PACKAGES).into_assume_dir())
    }

    /// Returns the directory where git repositories are cached.
    pub fn git(&self) -> AbsPresumedDirPathBuf {
        self.git
            .clone()
            .unwrap_or_else(|| self.root.join(consts::CACHED_GIT_DIR).into_assume_dir())
    }

    /// Returns the directory where url based sources are cached.
    pub fn url(&self) -> AbsPresumedDirPathBuf {
        self.url
            .clone()
            .unwrap_or_else(|| self.root.join(consts::CACHED_URL_DIR).into_assume_dir())
    }

    /// Returns the backend-metadata root.
    ///
    /// Layout under `<workspace or root>/meta-v0/`:
    ///
    /// ```text
    /// <source_unique_key>/
    ///   <host>-<meta_hash>.json          # cache entries (written by
    ///   <host>-<meta_hash>.revision       # BuildBackendMetadataCache)
    ///   <host>-<meta_hash>-files
    ///   work/<backend_scratch>/          # scratch passed to the
    ///                                    # backend's conda/outputs call
    /// ```
    ///
    /// The per-source `work/` subdir is reached via
    /// [`Self::backend_metadata_work_dir`]; the cache entries and the
    /// backend's scratch space for the same source live in the same
    /// tree so `rm -rf meta-v0/<source>/` wipes both.
    pub fn backend_metadata(&self) -> AbsPresumedDirPathBuf {
        self.build_backend_metadata.clone().unwrap_or_else(|| {
            self.workspace_or_root()
                .join(format!(
                    "{}-{}",
                    consts::CACHED_BUILD_BACKEND_METADATA,
                    BuildBackendMetadataCache::CACHE_SUFFIX
                ))
                .into_assume_dir()
        })
    }

    /// Returns the backend's scratch directory for the given source's
    /// `conda/outputs` call.
    ///
    /// Lives as a `work/` subdir inside that source's metadata cache
    /// dir, so the cache entries and the backend's scratch for the
    /// same source are grouped. `source_unique_key` comes from
    /// [`crate::build::CanonicalSourceCodeLocation::cache_unique_key`].
    pub fn backend_metadata_work_dir(&self, source_unique_key: &str) -> AbsPresumedDirPathBuf {
        self.backend_metadata()
            .join(source_unique_key)
            .join(consts::BACKEND_METADATA_WORK_SUBDIR)
            .into_assume_dir()
    }

    /// Returns the root for content-addressed source-build artifacts.
    ///
    /// Layout: `<workspace or root>/artifacts-v0/<pkg>/<cache_key>/`
    /// (managed by [`crate::cache::artifact::ArtifactCache`]).
    ///
    /// Rooted directly on the workspace (not under the `build/` dir)
    /// so Windows paths stay short.
    pub fn source_build_artifacts(&self) -> AbsPresumedDirPathBuf {
        self.source_build_artifacts.clone().unwrap_or_else(|| {
            self.workspace_or_root()
                .join(consts::SOURCE_BUILD_ARTIFACTS_DIR)
                .into_assume_dir()
        })
    }

    /// Returns the root for per-build backend workspaces (backend
    /// incremental state).
    ///
    /// Layout: `<workspace or root>/bld/<pkg>/<workspace_key>/`
    /// (managed by [`crate::cache::workspace::WorkspaceCache`]).
    ///
    /// Rooted directly on the workspace (not under `build/`) and named
    /// tersely so the deep nested backend directories fit under
    /// Windows' `MAX_PATH = 260` limit.
    pub fn source_build_workspaces(&self) -> AbsPresumedDirPathBuf {
        self.source_build_workspaces.clone().unwrap_or_else(|| {
            self.workspace_or_root()
                .join(consts::SOURCE_BUILD_WORKSPACES_DIR)
                .into_assume_dir()
        })
    }

    /// Returns the directory holding cached legacy (pre-v7) source
    /// build/host environments.
    ///
    /// Layout: `<workspace or root>/legacy-source-env/<hash>.json`
    /// (managed by
    /// `pixi_core::lock_file::satisfiability::legacy::cache`).
    pub fn legacy_source_env(&self) -> AbsPresumedDirPathBuf {
        self.workspace_or_root()
            .join(consts::LEGACY_SOURCE_ENV_DIR)
            .into_assume_dir()
    }

    /// Helper: the `.pixi/` workspace directory when set, otherwise the
    /// global cache root. Distinct from `build()`, which unconditionally
    /// appends `build/` to the workspace path.
    fn workspace_or_root(&self) -> AbsPresumedDirPathBuf {
        self.workspace.clone().unwrap_or_else(|| self.root.clone())
    }
}
