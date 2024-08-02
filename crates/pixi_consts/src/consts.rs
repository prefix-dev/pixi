use console::Style;
use lazy_static::lazy_static;
use std::fmt::{Display, Formatter};
use url::Url;

pub const DEFAULT_ENVIRONMENT_NAME: &str = "default";
pub const DEFAULT_FEATURE_NAME: &str = DEFAULT_ENVIRONMENT_NAME;
pub const PYPROJECT_PIXI_PREFIX: &str = "tool.pixi";

pub const PROJECT_MANIFEST: &str = "pixi.toml";
pub const PYPROJECT_MANIFEST: &str = "pyproject.toml";
pub const PROJECT_LOCK_FILE: &str = "pixi.lock";
pub const CONFIG_FILE: &str = "config.toml";
pub const PIXI_DIR: &str = ".pixi";
pub const PIXI_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PREFIX_FILE_NAME: &str = "pixi_env_prefix";
pub const ENVIRONMENTS_DIR: &str = "envs";
pub const SOLVE_GROUP_ENVIRONMENTS_DIR: &str = "solve-group-envs";
pub const PYPI_DEPENDENCIES: &str = "pypi-dependencies";
pub const TASK_CACHE_DIR: &str = "task-cache-v0";
pub const PIXI_UV_INSTALLER: &str = "uv-pixi";
pub const PYPI_CACHE_DIR: &str = "uv-cache";
pub const CONDA_INSTALLER: &str = "conda";

pub const ONE_TIME_MESSAGES_DIR: &str = "one-time-messages";

/// The default channels to use for a new project.
pub const DEFAULT_CHANNELS: &[&str] = &["conda-forge"];

pub const ENVIRONMENT_FILE_NAME: &str = "pixi";

lazy_static! {
    pub static ref TASK_STYLE: Style = Style::new().blue();
    pub static ref PLATFORM_STYLE: Style = Style::new().yellow();
    pub static ref ENVIRONMENT_STYLE: Style = Style::new().magenta();
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
