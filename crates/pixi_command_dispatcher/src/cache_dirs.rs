use crate::build::BuildCache;
use crate::cache::build_backend_metadata::BuildBackendMetadataCache;
use crate::cache::source_metadata::SourceMetadataCache;
use pixi_consts::consts;
use pixi_path::{AbsPresumedDirPath, AbsPresumedDirPathBuf};

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

    /// The directory where working directory for builds are cached.
    work_dirs: Option<AbsPresumedDirPathBuf>,

    /// The directory where binary packages are cached.
    packages: Option<AbsPresumedDirPathBuf>,

    /// The directory where git repositories are cached.
    git: Option<AbsPresumedDirPathBuf>,

    /// The location where to store build backend metadata information.
    build_backend_metadata: Option<AbsPresumedDirPathBuf>,

    /// The directory where url archives are cached.
    url: Option<AbsPresumedDirPathBuf>,

    /// The location where to store source metadata information.
    source_metadata: Option<AbsPresumedDirPathBuf>,

    /// The location where to store source builds.
    source_builds: Option<AbsPresumedDirPathBuf>,
}

impl CacheDirs {
    /// Instantiate a new `CacheDirs` instance with the given root directory.
    pub fn new(root: AbsPresumedDirPathBuf) -> Self {
        Self {
            root,
            workspace: None,
            build_backends: None,
            work_dirs: None,
            packages: None,
            git: None,
            build_backend_metadata: None,
            url: None,
            source_metadata: None,
            source_builds: None,
        }
    }

    pub fn with_workspace(self, workspace: AbsPresumedDirPathBuf) -> Self {
        Self {
            workspace: Some(workspace),
            ..self
        }
    }

    /// Sets the directory where source builds
    pub fn with_working_dirs(self, working_dirs: AbsPresumedDirPathBuf) -> Self {
        Self {
            work_dirs: Some(working_dirs),
            ..self
        }
    }

    /// Sets the working directories for builds.
    pub fn set_working_dirs(&mut self, working_dirs: AbsPresumedDirPathBuf) {
        self.work_dirs = Some(working_dirs);
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

    /// Returns the directory that is the root directory to store workspace
    /// build related caches.
    pub fn build(&self) -> AbsPresumedDirPathBuf {
        self.workspace()
            .map(|workspace| {
                workspace
                    .join(consts::WORKSPACE_CACHE_DIR)
                    .into_assume_dir()
            })
            .unwrap_or_else(|| self.root.clone())
    }

    /// Returns the directory where build backend environments are cached.
    pub fn build_backends(&self) -> AbsPresumedDirPathBuf {
        self.build_backends.clone().unwrap_or_else(|| {
            self.root
                .join(consts::CACHED_BUILD_BACKENDS)
                .into_assume_dir()
        })
    }

    /// Returns the directory where working directories are cached.
    pub fn working_dirs(&self) -> AbsPresumedDirPathBuf {
        self.work_dirs.clone().unwrap_or_else(|| {
            self.build()
                .join(consts::CACHED_BUILD_WORK_DIR)
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

    /// Returns the directory where git repositories are cached.
    pub fn url(&self) -> AbsPresumedDirPathBuf {
        self.url
            .clone()
            .unwrap_or_else(|| self.root.join(consts::CACHED_URL_DIR).into_assume_dir())
    }

    /// Returns the directory where source metadata is cached.
    pub fn build_backend_metadata(&self) -> AbsPresumedDirPathBuf {
        self.build_backend_metadata.clone().unwrap_or_else(|| {
            self.build()
                .join(format!(
                    "{}-{}",
                    consts::CACHED_BUILD_BACKEND_METADATA,
                    BuildBackendMetadataCache::CACHE_SUFFIX
                ))
                .into_assume_dir()
        })
    }

    /// Returns the directory where source metadata is cached.
    pub fn source_metadata(&self) -> AbsPresumedDirPathBuf {
        self.source_metadata.clone().unwrap_or_else(|| {
            self.build()
                .join(format!(
                    "{}-{}",
                    consts::CACHED_SOURCE_METADATA,
                    SourceMetadataCache::CACHE_SUFFIX
                ))
                .into_assume_dir()
        })
    }

    /// Returns the directory where source builds are cached.
    pub fn source_builds(&self) -> AbsPresumedDirPathBuf {
        self.source_builds.clone().unwrap_or_else(|| {
            self.build()
                .join(format!(
                    "{}-{}",
                    consts::CACHED_SOURCE_BUILDS,
                    BuildCache::CACHE_SUFFIX
                ))
                .into_assume_dir()
        })
    }
}
