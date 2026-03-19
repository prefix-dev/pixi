use std::path::PathBuf;

use minijinja::Environment;
use serde::Serialize;

#[derive(Serialize)]
pub struct BuildScriptContext {
    pub installer: Installer,
    pub build_platform: BuildPlatform,
    pub editable: bool,
    pub extra_args: Vec<String>,
    pub manifest_root: PathBuf,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Installer {
    Pip,
    #[default]
    Uv,
}

impl Installer {
    pub fn package_name(&self) -> &str {
        match self {
            Installer::Uv => "uv",
            Installer::Pip => "pip",
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildPlatform {
    Windows,
    Unix,
}

impl BuildScriptContext {
    pub fn render(&self) -> String {
        let env = Environment::new();
        let template = env
            .template_from_str(include_str!("build_script.j2"))
            .unwrap();
        template.render(self).unwrap().trim().to_string()
    }
}
