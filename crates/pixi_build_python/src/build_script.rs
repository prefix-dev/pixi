use std::path::PathBuf;

use minijinja::Environment;
use pixi_build_types::SourcePackageName;
use serde::Serialize;

const UV: &str = "uv";
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

    /// Determine the installer from an iterator of dependency package names.
    ///
    /// `uv` is the default installer. `pip` is selected only when `pip` is
    /// present in the dependencies and `uv` is not. If both are present, `uv`
    /// wins.
    pub fn determine_installer_from_names<'a>(
        package_names: impl Iterator<Item = &'a str>,
    ) -> Installer {
        let mut has_uv = false;
        let mut has_pip = false;
        for name in package_names {
            if name == UV {
                has_uv = true;
            } else if name == PIP {
                has_pip = true;
            }
        }

        if has_pip && !has_uv {
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
