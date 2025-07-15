use std::path::PathBuf;

use pixi_consts::consts;

use crate::build::{BuildCache, source_metadata_cache::SourceMetadataCache};

pub struct CacheDirs {
    /// The root cache directory, all other cache directories are derived from
    /// this.
    root: PathBuf,

    /// Specifies the cache directory for workspace specific directories. If
    /// this is not specified the directories are stored in the global
    /// cache.
    workspace: Option<PathBuf>,

    /// Directory where environments for build backends are cached.
    build_backends: Option<PathBuf>,

    /// The directory where working directory for builds are cached.
    work_dirs: Option<PathBuf>,

    /// The directory where binary packages are cached.
    packages: Option<PathBuf>,

    /// The directory where git repositories are cached.
    git: Option<PathBuf>,

    /// The location where to store source metadata information.
    source_metadata: Option<PathBuf>,

    /// The location where to store source builds.
    source_builds: Option<PathBuf>,
}

impl CacheDirs {
    /// Instantiate a new `CacheDirs` instance with the given root directory.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            workspace: None,
            build_backends: None,
            work_dirs: None,
            packages: None,
            git: None,
            source_metadata: None,
            source_builds: None,
        }
    }

    pub fn with_workspace(self, workspace: PathBuf) -> Self {
        Self {
            workspace: Some(workspace),
            ..self
        }
    }

    /// Returns the root directory for the cache.
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Returns the workspace directory if it is set, otherwise returns `None`.
    /// For pixi this would refer to the `.pixi` directory in the workspace.
    pub fn workspace(&self) -> Option<&PathBuf> {
        self.workspace.as_ref()
    }

    /// Returns the directory that is the root directory to store workspace
    /// build related caches.
    pub fn build(&self) -> PathBuf {
        self.workspace()
            .map(|workspace| workspace.join(consts::WORKSPACE_CACHE_DIR))
            .unwrap_or_else(|| self.root.clone())
    }

    /// Returns the directory where build backend environments are cached.
    pub fn build_backends(&self) -> PathBuf {
        self.build_backends
            .clone()
            .unwrap_or_else(|| self.root.join(consts::CACHED_BUILD_BACKENDS))
    }

    /// Returns the directory where working directories are cached.
    pub fn working_dirs(&self) -> PathBuf {
        self.work_dirs
            .clone()
            .unwrap_or_else(|| self.build().join(consts::CACHED_BUILD_WORK_DIR))
    }

    /// Returns the location to store packages
    pub fn packages(&self) -> PathBuf {
        self.packages
            .clone()
            .unwrap_or_else(|| self.root.join(consts::CACHED_PACKAGES))
    }

    /// Returns the directory where git repositories are cached.
    pub fn git(&self) -> PathBuf {
        self.git
            .clone()
            .unwrap_or_else(|| self.root.join(consts::CACHED_GIT_DIR))
    }

    /// Returns the directory where source metadata is cached.
    pub fn source_metadata(&self) -> PathBuf {
        self.source_metadata.clone().unwrap_or_else(|| {
            self.build().join(format!(
                "{}-{}",
                consts::CACHED_SOURCE_METADATA,
                SourceMetadataCache::CACHE_SUFFIX
            ))
        })
    }

    /// Returns the directory where source builds are cached.
    pub fn source_builds(&self) -> PathBuf {
        self.source_builds.clone().unwrap_or_else(|| {
            self.build().join(format!(
                "{}-{}",
                consts::CACHED_SOURCE_BUILDS,
                BuildCache::CACHE_SUFFIX
            ))
        })
    }
}
