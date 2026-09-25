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

/// Match the logging target configured by the backend CLI for build output.
pub fn verbosity() -> u8 {
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
        template.render(self).unwrap().trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildPlatform, BuildScriptContext, Installer, verbosity};

    #[test]
    fn installer_verbosity_follows_backend_logging() {
        for installer in [Installer::Uv, Installer::Pip] {
            for windows in [false, true] {
                for (level, expected) in
                    [("off", ""), ("info", ""), ("debug", "-v"), ("trace", "-vv")]
                {
                    use tracing_subscriber::prelude::*;
                    let subscriber = tracing_subscriber::registry().with(
                        tracing_subscriber::EnvFilter::new(format!("warn,rattler_build={level}")),
                    );
                    let verbosity = tracing::subscriber::with_default(subscriber, verbosity);
                    let context = BuildScriptContext {
                        installer: installer.clone(),
                        build_platform: if windows {
                            BuildPlatform::Windows
                        } else {
                            BuildPlatform::Unix
                        },
                        editable: true,
                        extra_args: vec!["--config-settings=foo=bar".into()],
                        manifest_root: "source directory".into(),
                        verbosity,
                    };
                    let script = context.render();
                    let flags: Vec<_> = script
                        .split_whitespace()
                        .filter(|word| word.starts_with("-v"))
                        .collect();
                    let expected_flags: Vec<_> = expected.split_whitespace().collect();
                    assert_eq!(flags, expected_flags, "{installer:?}: {script}");
                    for arg in [
                        "--no-deps",
                        "--no-build-isolation",
                        "--no-index",
                        "--editable",
                        "--config-settings=foo=bar",
                        "\"source directory\"",
                    ] {
                        assert!(script.contains(arg), "missing {arg}: {script}");
                    }
                    assert_eq!(script.contains("if errorlevel 1 exit 1"), windows);
                    match installer {
                        Installer::Uv => assert!(script.starts_with("uv pip install")),
                        Installer::Pip => assert!(script.contains("-m pip install")),
                    }
                }
            }
        }
    }
}
