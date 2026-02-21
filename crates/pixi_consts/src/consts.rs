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

pub const WORKSPACE_MANIFEST: &str = "pixi.toml";
pub const PYPROJECT_MANIFEST: &str = "pyproject.toml";
pub const CONFIG_FILE: &str = "config.toml";
pub const PIXI_VERSION: &str = match option_env!("PIXI_VERSION") {
    Some(v) => v,
    None => "0.63.2",
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
pub const CONDA_MENU_SCHEMA_DIR: &str = "Menu";
pub const PYPI_CACHE_DIR: &str = "uv-cache";
pub const CONDA_PYPI_MAPPING_CACHE_DIR: &str = "conda-pypi-mapping";
pub const CACHED_ENVS_DIR: &str = "cached-envs-v0";
// TODO: CACHED_BUILD_ENVS_DIR was deprecated in favor of CACHED_BUILD_TOOL_ENVS_DIR. This constant will be removed in a future release.
pub const _CACHED_BUILD_ENVS_DIR: &str = "cached-build-envs-v0";
pub const CACHED_BUILD_TOOL_ENVS_DIR: &str = "cached-build-tool-envs-v0";
pub const CACHED_GIT_DIR: &str = "git-v0";
pub const CACHED_URL_DIR: &str = "url-v0";
pub const CACHED_BUILD_WORK_DIR: &str = "work";
pub const CACHED_BUILD_BACKENDS: &str = "backends-v0";
pub const CACHED_PACKAGES: &str = "pkgs";
pub const CACHED_BUILD_BACKEND_METADATA: &str = "metadata";
pub const CACHED_SOURCE_METADATA: &str = "source_metadata";
pub const CACHED_SOURCE_BUILDS: &str = "pkgs";
pub const WORKSPACES_REGISTRY: &str = "workspaces.toml";

/// The directory relative to the .pixi folder that stores build related caches.
pub const WORKSPACE_CACHE_DIR: &str = "build";

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
            vec![
                NamedChannelOrUrl::from_str("conda-forge")
                    .expect("unable to parse default channel"),
            ]
        }
    });

pub const MOJOPROJECT_MANIFEST: &str = "mojoproject.toml";

pub const CONDA_INSTALLER: &str = "conda";

pub const ONE_TIME_MESSAGES_DIR: &str = "one-time-messages";

pub const ENVIRONMENT_FILE_NAME: &str = "pixi";

// Note: no trailing slash!
pub const RELEASES_URL: &str = "https://github.com/prefix-dev/pixi/releases";
pub const RELEASES_API_BY_TAG: &str = "https://api.github.com/repos/prefix-dev/pixi/releases/tags";
pub const RELEASES_API_LATEST: &str =
    "https://api.github.com/repos/prefix-dev/pixi/releases/latest";

pub const CLAP_CONFIG_OPTIONS: &str = "Config Options";
pub const CLAP_GIT_OPTIONS: &str = "Git Options";
pub const CLAP_GLOBAL_OPTIONS: &str = "Global Options";
pub const CLAP_UPDATE_OPTIONS: &str = "Update Options";

// Build backends constants
pub const RATTLER_BUILD_FILE_NAMES: [&str; 2] = ["recipe.yaml", "recipe.yml"];
pub const RATTLER_BUILD_DIRS: [&str; 2] = ["", "recipe"];
pub const ROS_BACKEND_FILE_NAMES: [&str; 1] = ["package.xml"];

pub static KNOWN_MANIFEST_FILES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut v = Vec::new();
    v.push(WORKSPACE_MANIFEST);
    v.push(PYPROJECT_MANIFEST);
    v.push(MOJOPROJECT_MANIFEST);
    v.extend(RATTLER_BUILD_FILE_NAMES);
    v
});
pub static TASK_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().blue());
pub static TASK_ERROR_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().red());
pub static PLATFORM_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().yellow());
pub static ENVIRONMENT_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().magenta());
pub static EXPOSED_NAME_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().yellow());
pub static FEATURE_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().cyan());
pub static SOLVE_GROUP_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().cyan());
pub static CONDA_PACKAGE_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().green());
pub static PYPI_PACKAGE_STYLE: LazyLock<Style> = LazyLock::new(|| Style::new().blue());
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
