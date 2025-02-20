use console::Style;
use rattler_conda_types::NamedChannelOrUrl;
use std::{
    fmt::{Display, Formatter},
    str::FromStr,
    sync::LazyLock,
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
    None => "0.41.4",
};
pub const PREFIX_FILE_NAME: &str = "pixi_env_prefix";
pub const ENVIRONMENTS_DIR: &str = "envs";
pub const SOLVE_GROUP_ENVIRONMENTS_DIR: &str = "solve-group-envs";
pub const PYPI_DEPENDENCIES: &str = "pypi-dependencies";
pub const DEPENDENCIES: &str = "dependencies";
pub const SYSTEM_REQUIREMENTS: &str = "system-requirements";
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

/// The default config directory for pixi, typically at $XDG_CONFIG_HOME/$PIXI_CONFIG_DIR or $HOME/.config/$PIXI_CONFIG_DIR.
pub const CONFIG_DIR: &str = match option_env!("PIXI_CONFIG_DIR") {
    Some(dir) => dir,
    None => "pixi",
};
/// The default file name for the lock file in a project.
pub const PROJECT_LOCK_FILE: &str = match option_env!("PIXI_PROJECT_LOCK_FILE") {
    Some(file) => file,
    None => "pixi.lock",
};
/// The default directory for the pixi files in a project.
pub const PIXI_DIR: &str = match option_env!("PIXI_DIR") {
    Some(dir) => dir,
    None => ".pixi",
};
/// The default manifest name for the global manifest file in the pixi config directory.
pub const GLOBAL_MANIFEST_DEFAULT_NAME: &str =
    match option_env!("PIXI_GLOBAL_MANIFEST_DEFAULT_NAME") {
        Some(name) => name,
        None => "pixi-global.toml",
    };

pub static DEFAULT_CHANNELS: LazyLock<Vec<NamedChannelOrUrl>> =
    LazyLock::new(|| match option_env!("PIXI_DEFAULT_CHANNELS") {
        // Technically URLs are allowed to contain ','
        // If that use case comes up, this code needs to be adapted
        Some(channels) => channels
            .split(',')
            .map(|s| NamedChannelOrUrl::from_str(s).expect("unable to parse default channel"))
            .collect(),
        None => {
            vec![NamedChannelOrUrl::from_str("conda-forge")
                .expect("unable to parse default channel")]
        }
    });

pub const CONDA_INSTALLER: &str = "conda";

pub const ONE_TIME_MESSAGES_DIR: &str = "one-time-messages";

pub const ENVIRONMENT_FILE_NAME: &str = "pixi";

pub const RELEASES_URL: &str = "https://github.com/prefix-dev/pixi/releases";

pub static TASK_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().blue());
pub static PLATFORM_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().yellow());
pub static ENVIRONMENT_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().magenta());
pub static EXPOSED_NAME_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().yellow());
pub static FEATURE_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().cyan());
pub static SOLVE_GROUP_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().cyan());
pub static DEFAULT_PYPI_INDEX_URL: LazyLock<Url> =
    LazyLock::new(|| Url::parse("https://pypi.org/simple").unwrap());

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
