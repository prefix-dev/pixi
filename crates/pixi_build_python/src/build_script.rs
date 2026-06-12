use std::path::PathBuf;

use minijinja::Environment;
use pixi_build_types::SourcePackageName;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct BuildScriptContext {
    pub installer: Installer,
    pub build_platform: BuildPlatform,
    pub editable: bool,
    pub extra_args: Vec<String>,
    pub manifest_root: PathBuf,
}

/// The Python package installer used in the build script.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Installer {
    #[default]
    Uv,
    Pip,
}

impl Installer {
    pub fn package_name(&self) -> SourcePackageName {
        match self {
            Installer::Uv => rattler_conda_types::PackageName::new_unchecked("uv").into(),
            Installer::Pip => rattler_conda_types::PackageName::new_unchecked("pip").into(),
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
