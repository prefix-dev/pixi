use super::config::{MojoBinConfig, MojoPkgConfig};
use minijinja::Environment;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct BuildScriptContext {
    /// Any executable artifacts to create.
    pub bins: Option<Vec<MojoBinConfig>>,
    /// Any packages to create.
    pub pkg: Option<MojoPkgConfig>,
    /// The package format, either a concrete value or a Jinja variant expression.
    pub pkg_format: String,
    /// Whether the build host is Windows.
    pub is_windows: bool,
}

impl BuildScriptContext {
    pub fn render(&self) -> String {
        let env = Environment::new();
        let template = env
            .template_from_str(include_str!("build_script.j2"))
            .unwrap();
        // Normalize line endings to Unix-style for consistent output across platforms
        template
            .render(self)
            .unwrap()
            .trim()
            .replace("\r\n", "\n")
            .to_string()
    }
}
