use std::path::PathBuf;

use pixi_consts::consts::{
    CACHED_BUILD_BACKENDS, CACHED_BUILD_WORK_DIR, CACHED_GIT_DIR, CACHED_PACKAGES,
};

pub struct CacheDirs {
    /// The root cache directory, all other cache directories are derived from
    /// this.
    root: PathBuf,

    /// The cache directory for build backends.
    build_backends: Option<PathBuf>,

    /// Working directories for build backends.
    work_dirs: Option<PathBuf>,

    /// The cache directory for packages.
    packages: Option<PathBuf>,

    /// The cache directory for git.
    git: Option<PathBuf>,
}

impl CacheDirs {
    /// Instantiate a new `CacheDirs` instance with the given root directory.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            build_backends: None,
            work_dirs: None,
            packages: None,
            git: None,
        }
    }

    /// Returns the root directory for the cache.
    pub fn root(&self) -> &PathBuf {
        &self.root
    }

    /// Returns the directory where build backend environments are cached.
    pub fn build_backends(&self) -> PathBuf {
        self.build_backends
            .clone()
            .unwrap_or_else(|| self.root.join(CACHED_BUILD_BACKENDS))
    }

    /// Returns the directory where working directories are cached.
    pub fn working_dirs(&self) -> PathBuf {
        self.work_dirs
            .clone()
            .unwrap_or_else(|| self.root.join(CACHED_BUILD_WORK_DIR))
    }

    /// Returns the location to store packages
    pub fn packages(&self) -> PathBuf {
        self.packages
            .clone()
            .unwrap_or_else(|| self.root.join(CACHED_PACKAGES))
    }

    pub fn git(&self) -> PathBuf {
        self.git
            .clone()
            .unwrap_or_else(|| self.root.join(CACHED_GIT_DIR))
    }

    /// Sets the working directory for build backends.
    pub fn with_working_dirs(self, work_dirs: PathBuf) -> Self {
        Self {
            work_dirs: Some(work_dirs),
            ..self
        }
    }
}
