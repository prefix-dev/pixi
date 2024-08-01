use url::Url;

pub const PYPI_DEPENDENCIES: &str = "pypi-dependencies";
pub const DEFAULT_ENVIRONMENT_NAME: &str = "default";
pub const PROJECT_MANIFEST: &str = "pixi.toml";
pub const PYPROJECT_MANIFEST: &str = "pyproject.toml";
pub const DEFAULT_FEATURE_NAME: &str = DEFAULT_ENVIRONMENT_NAME;
pub const PYPROJECT_PIXI_PREFIX: &str = "tool.pixi";

lazy_static::lazy_static! {
    pub static ref DEFAULT_PYPI_INDEX_URL: Url = Url::parse("https://pypi.org/simple").unwrap();
}
