use std::path::PathBuf;

use minijinja::Environment;
use pixi_build_types::SourcePackageName;
use serde::Serialize;

const PIP: &str = "pip";
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
    pub fn package_name(&self) -> SourcePackageName {
        match self {
            Installer::Uv => rattler_conda_types::PackageName::new_unchecked("uv").into(),
            Installer::Pip => rattler_conda_types::PackageName::new_unchecked("pip").into(),
        }
    }

    /// Determine the installer from an iterator of dependency package names.
    /// Checks if "uv" is present in the package names.
    pub fn determine_installer_from_names<'a>(
        mut package_names: impl Iterator<Item = &'a str>,
    ) -> Installer {
        // Check all dependency names for "uv" package
        let has_pip = package_names.any(|name| name == PIP);

        if has_pip {
            Installer::Pip
        } else {
            Installer::Uv
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
