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
    /// How many `-v` flags to pass to the installer.
    pub verbosity: u8,
}

/// Installer verbosity matching the backend's build log level. The backend
/// applies its CLI log level to the `rattler_build` target, which also logs the
/// build script output.
pub fn installer_verbosity() -> u8 {
    if tracing::enabled!(target: "rattler_build", tracing::Level::TRACE) {
        2
    } else if tracing::enabled!(target: "rattler_build", tracing::Level::DEBUG) {
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
        template
            .render(minijinja::value::Serde(self))
            .unwrap()
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(installer: Installer, verbosity: u8) -> String {
        BuildScriptContext {
            installer,
            build_platform: BuildPlatform::Unix,
            editable: false,
            extra_args: vec![],
            manifest_root: "/src".into(),
            verbosity,
        }
        .render()
    }

    #[test]
    fn installer_verbosity_flags() {
        let scripts = [Installer::Uv, Installer::Pip]
            .into_iter()
            .flat_map(|installer| {
                (0..=2).map(move |verbosity| render(installer.clone(), verbosity))
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        insta::assert_snapshot!(scripts, @r#"
        uv pip install --python "$PYTHON" --reinstall --no-deps \
                --no-build-isolation \
                --no-index "/src"

        uv pip install --python "$PYTHON" --reinstall -v \
                --no-deps \
                --no-build-isolation \
                --no-index "/src"

        uv pip install --python "$PYTHON" --reinstall -vv \
                --no-deps \
                --no-build-isolation \
                --no-index "/src"

        "$PYTHON" -m pip install --force-reinstall --no-deps \
                --no-build-isolation \
                --no-index "/src"

        "$PYTHON" -m pip install --force-reinstall -v \
                --no-deps \
                --no-build-isolation \
                --no-index "/src"

        "$PYTHON" -m pip install --force-reinstall -vv \
                --no-deps \
                --no-build-isolation \
                --no-index "/src"
        "#);
    }
}
