use console::Style;
use lazy_static::lazy_static;

pub const PROJECT_MANIFEST: &str = "pixi.toml";
pub const PROJECT_LOCK_FILE: &str = "pixi.lock";
pub const PIXI_DIR: &str = ".pixi";
pub const PREFIX_FILE_NAME: &str = "prefix";
pub const ENVIRONMENTS_DIR: &str = "envs";
pub const PYPI_DEPENDENCIES: &str = "pypi-dependencies";

pub const DEFAULT_ENVIRONMENT_NAME: &str = "default";

pub const DEFAULT_FEATURE_NAME: &str = DEFAULT_ENVIRONMENT_NAME;

lazy_static! {
    pub static ref ENV_STYLE: Style = Style::new().magenta();
    pub static ref FEAT_STYLE: Style = Style::new().cyan();
    pub static ref TASK_STYLE: Style = Style::new().blue();
}
