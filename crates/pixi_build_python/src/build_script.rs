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
    pub verbosity: u8,
}

pub fn verbosity(debug_enabled: bool, trace_enabled: bool) -> u8 {
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
    use super::{BuildPlatform, BuildScriptContext, Installer, verbosity};

    fn context() -> BuildScriptContext {
        BuildScriptContext {
            installer: Installer::Uv,
            build_platform: BuildPlatform::Unix,
            editable: false,
            extra_args: Vec::new(),
            manifest_root: "/source".into(),
            verbosity: 0,
        }
    }

    #[test]
    fn installer_is_quiet_without_explicit_verbosity() {
        let script = context().render();
        assert!(!script.contains(" -v"), "unexpected verbose flag: {script}");
    }

    #[test]
    fn uv_installer_verbosity_follows_backend_logging() {
        let mut context = context();
        context.verbosity = 1;
        assert!(context.render().contains("--reinstall -v "));

        context.verbosity = 2;
        assert!(context.render().contains("--reinstall -vv "));
    }

    #[test]
    fn pip_installer_verbosity_follows_backend_logging() {
        let mut context = context();
        context.installer = Installer::Pip;
        assert!(
            !context.render().contains(" -v"),
            "unexpected default verbose flag: {}",
            context.render()
        );

        context.verbosity = 1;
        assert!(
            context
                .render()
                .contains("pip install --force-reinstall -v ")
        );

        context.verbosity = 2;
        assert!(
            context
                .render()
                .contains("pip install --force-reinstall -vv ")
        );
    }

    #[test]
    fn verbosity_is_derived_from_backend_logging() {
        assert_eq!(verbosity(false, false), 0);
        assert_eq!(verbosity(true, false), 1);
        assert_eq!(verbosity(true, true), 2);
    }

    #[test]
    fn pip_extra_args_are_preserved() {
        let mut context = context();
        context.installer = Installer::Pip;
        context.extra_args.push("--config-settings=foo=bar".into());
        assert!(
            context
                .render()
                .contains("pip install --force-reinstall --no-deps")
        );
        assert!(context.render().contains("--config-settings=foo=bar"));
    }
}
