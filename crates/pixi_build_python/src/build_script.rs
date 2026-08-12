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
    pub uv_verbosity: u8,
}

pub fn uv_verbosity(debug_enabled: bool, trace_enabled: bool) -> u8 {
    if trace_enabled {
        2
    } else if debug_enabled {
        1
    } else {
        0
    }
}

/// The tool used to install the built wheel into the prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::{BuildPlatform, BuildScriptContext, Installer, uv_verbosity};

    fn context() -> BuildScriptContext {
        BuildScriptContext {
            installer: Installer::Uv,
            build_platform: BuildPlatform::Unix,
            editable: false,
            extra_args: Vec::new(),
            manifest_root: "/source".into(),
            uv_verbosity: 0,
        }
    }

    #[test]
    fn uv_is_quiet_without_explicit_verbosity() {
        let script = context().render();
        assert!(!script.contains(" -v"), "unexpected verbose flag: {script}");
    }

    #[test]
    fn uv_verbosity_follows_backend_logging() {
        let mut context = context();
        context.uv_verbosity = 1;
        assert!(context.render().contains("--reinstall -v "));

        context.uv_verbosity = 2;
        assert!(context.render().contains("--reinstall -vv "));
    }

    #[test]
    fn uv_verbosity_is_derived_from_backend_logging() {
        assert_eq!(uv_verbosity(false, false), 0);
        assert_eq!(uv_verbosity(true, false), 1);
        assert_eq!(uv_verbosity(true, true), 2);
    }

    #[test]
    fn pip_keeps_its_existing_verbosity() {
        let mut context = context();
        context.installer = Installer::Pip;
        assert!(
            context
                .render()
                .contains("pip install --force-reinstall -vv ")
        );
    }
}
