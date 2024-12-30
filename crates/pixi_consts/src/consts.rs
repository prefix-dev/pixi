use console::Style;
use lazy_static::lazy_static;
use std::{
    ffi::OsStr,
    fmt::{Display, Formatter},
    path::Path,
};
use url::Url;

pub const DEFAULT_ENVIRONMENT_NAME: &str = "default";
pub const DEFAULT_FEATURE_NAME: &str = DEFAULT_ENVIRONMENT_NAME;
pub const PYPROJECT_PIXI_PREFIX: &str = "tool.pixi";

pub const PROJECT_MANIFEST: &str = "pixi.toml";
pub const PYPROJECT_MANIFEST: &str = "pyproject.toml";
pub const CONFIG_FILE: &str = "config.toml";
pub const PIXI_VERSION: &str = match option_env!("PIXI_VERSION") {
    Some(v) => v,
    None => "0.39.4",
};
pub const PREFIX_FILE_NAME: &str = "pixi_env_prefix";
pub const ENVIRONMENTS_DIR: &str = "envs";
pub const SOLVE_GROUP_ENVIRONMENTS_DIR: &str = "solve-group-envs";
pub const PYPI_DEPENDENCIES: &str = "pypi-dependencies";
pub const DEPENDENCIES: &str = "dependencies";
pub const TASK_CACHE_DIR: &str = "task-cache-v0";
pub const ACTIVATION_ENV_CACHE_DIR: &str = "activation-env-v0";
pub const PIXI_UV_INSTALLER: &str = "uv-pixi";
pub const CONDA_PACKAGE_CACHE_DIR: &str = rattler_cache::PACKAGE_CACHE_DIR;
pub const CONDA_REPODATA_CACHE_DIR: &str = rattler_cache::REPODATA_CACHE_DIR;
// TODO: move to rattler
pub const CONDA_META_DIR: &str = "conda-meta";
pub const PYPI_CACHE_DIR: &str = "uv-cache";
pub const CONDA_PYPI_MAPPING_CACHE_DIR: &str = "conda-pypi-mapping";
pub const CACHED_ENVS_DIR: &str = "cached-envs-v0";
// TODO: CACHED_BUILD_ENVS_DIR was deprecated in favor of CACHED_BUILD_ENVS_DIR. This constant will be removed in a future release.
pub const _CACHED_BUILD_ENVS_DIR: &str = "cached-build-envs-v0";
pub const CACHED_BUILD_TOOL_ENVS_DIR: &str = "cached-build-tool-envs-v0";
pub const CACHED_GIT_DIR: &str = "git-cache-v0";

pub const CONFIG_DIR: &str = match option_env!("PIXI_CONFIG_DIR") {
    Some(dir) => dir,
    None => "pixi",
};
pub const PROJECT_LOCK_FILE: &str = match option_env!("PIXI_PROJECT_LOCK_FILE") {
    Some(file) => file,
    None => "pixi.lock",
};
pub const PIXI_DIR: &str = match option_env!("PIXI_DIR") {
    Some(dir) => dir,
    None => ".pixi",
};

lazy_static! {
    /// The default channels to use for a new project.
    pub static ref DEFAULT_CHANNELS: Vec<String> = match option_env!("PIXI_DEFAULT_CHANNELS") {
        Some(channels) => channels.split(',').map(|s| s.to_string()).collect(),
        None => vec!["conda-forge".to_string()],
    };

    /// The name of the binary.
    pub static ref PIXI_BIN_NAME: String = std::env::args().next()
        .as_ref()
        .map(Path::new)
        .and_then(Path::file_stem)
        .and_then(OsStr::to_str)
        .map(String::from).unwrap_or("pixi".to_string());
}

pub const CONDA_INSTALLER: &str = "conda";

pub const ONE_TIME_MESSAGES_DIR: &str = "one-time-messages";

pub const ENVIRONMENT_FILE_NAME: &str = "pixi";

lazy_static! {
    pub static ref TASK_STYLE: Style = Style::new().blue();
    pub static ref PLATFORM_STYLE: Style = Style::new().yellow();
    pub static ref ENVIRONMENT_STYLE: Style = Style::new().magenta();
    pub static ref EXPOSED_NAME_STYLE: Style = Style::new().yellow();
    pub static ref FEATURE_STYLE: Style = Style::new().cyan();
    pub static ref SOLVE_GROUP_STYLE: Style = Style::new().cyan();
    pub static ref DEFAULT_PYPI_INDEX_URL: Url = Url::parse("https://pypi.org/simple").unwrap();
}

pub struct CondaEmoji;

impl Display for CondaEmoji {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if console::Term::stderr().features().colors_supported() {
            write!(f, "{}", console::style("C").bold().green())
        } else {
            write!(f, "(conda)")
        }
    }
}

pub struct PypiEmoji;

impl Display for PypiEmoji {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if console::Term::stderr().features().colors_supported() {
            write!(f, "{}", console::style("P").bold().blue())
        } else {
            write!(f, "(pypi)")
        }
    }
}
