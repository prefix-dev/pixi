use crate::Workspace;
use fancy_display::FancyDisplay;
use itertools::Itertools;
use miette::{Diagnostic, LabeledSpan};
use pixi_manifest::{EnvironmentName, PixiPlatformName, TaskName};
use rattler_conda_types::{GenericVirtualPackage, Platform, Version};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use thiserror::Error;

/// An error that occurs when data is requested for a platform that is not supported.
#[derive(Debug, Clone)]
pub struct UnsupportedPlatformError {
    /// Platforms supported by the environment
    pub environments_platforms: Vec<PixiPlatformName>,

    /// The environment that the platform is not supported for.
    pub environment: EnvironmentName,

    /// The platform that was requested
    pub platform: Platform,

    /// Declared virtual packages from workspace platforms that match the
    /// host subdir but are not provided by this machine. Empty when the
    /// platform mismatch isn't caused by missing virtual packages -- for
    /// example, when the user explicitly asked for a platform the
    /// environment doesn't declare at all.
    pub unsatisfied_requirements: Vec<GenericVirtualPackage>,
}

impl Error for UnsupportedPlatformError {}

impl Display for UnsupportedPlatformError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let lead = match &self.environment {
            EnvironmentName::Default => {
                format!("The workspace does not support '{}'", self.platform)
            }
            EnvironmentName::Named(name) => {
                format!(
                    "the environment '{name}' does not support '{}'",
                    self.platform
                )
            }
        };
        if self.unsatisfied_requirements.is_empty() {
            match &self.environment {
                EnvironmentName::Default => write!(
                    f,
                    "{lead}.\nAdd it with 'pixi workspace platform add {}'.",
                    self.platform,
                ),
                EnvironmentName::Named(_) => write!(f, "{lead}"),
            }
        } else {
            write!(
                f,
                "{lead} on this machine:\n\
                no declared platform's virtual packages are satisfied here.\n\n\
                Unsatisfied requirements: {}",
                format_requirements(&self.unsatisfied_requirements),
            )
        }
    }
}

impl Diagnostic for UnsupportedPlatformError {
    fn code(&self) -> Option<Box<dyn Display + '_>> {
        Some(Box::new("unsupported-platform".to_string()))
    }

    fn help(&self) -> Option<Box<dyn Display + '_>> {
        let overrides: Vec<String> = self
            .unsatisfied_requirements
            .iter()
            .filter_map(|req| conda_override_hint(req.name.as_normalized(), Some(&req.version)))
            .collect();

        if overrides.is_empty() {
            Some(Box::new(format!(
                "supported platforms are {}",
                self.environments_platforms.iter().format(", ")
            )))
        } else {
            Some(Box::new(format!(
                "Mock the missing virtual packages via the environment, e.g.:\n  {}",
                overrides.join("\n  ")
            )))
        }
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        None
    }
}

fn format_requirements(reqs: &[GenericVirtualPackage]) -> String {
    reqs.iter()
        .map(|r| {
            // Version 0 encodes a version-less requirement (a bare `__cuda`
            // dependency): the package must be present at any version.
            if r.version == Version::major(0) {
                format!("{} (any version)", r.name.as_normalized())
            } else {
                format!("{} >= {}", r.name.as_normalized(), r.version)
            }
        })
        .join(", ")
}

/// `CONDA_OVERRIDE_*` hint for a missing virtual package: the required
/// version when known, a realistic example otherwise. `None` for virtual
/// packages without a known override (e.g. `__unix`).
pub(crate) fn conda_override_hint(name: &str, version: Option<&Version>) -> Option<String> {
    let env_var = match name {
        "__glibc" => "CONDA_OVERRIDE_GLIBC",
        "__cuda" => "CONDA_OVERRIDE_CUDA",
        "__osx" => "CONDA_OVERRIDE_OSX",
        "__linux" => "CONDA_OVERRIDE_LINUX",
        "__win" => "CONDA_OVERRIDE_WIN",
        "__archspec" => "CONDA_OVERRIDE_ARCHSPEC",
        _ => return None,
    };
    // A version-0 requirement means "any version"; "=0" reads like nonsense,
    // so suggest a realistic value instead.
    let example = match version.filter(|v| **v != Version::major(0)) {
        Some(version) => version.to_string(),
        None => match name {
            "__glibc" => "2.17".to_string(),
            "__cuda" => "12.0".to_string(),
            "__osx" => "10.15".to_string(),
            _ => "0".to_string(),
        },
    };
    Some(format!("{env_var}={example}"))
}

/// Errors that can occur while resolving workspace build variants.
#[derive(Debug, Diagnostic, Error)]
pub enum VariantsError {
    #[error("failed to read variant file '{path}'")]
    #[diagnostic(code(workspace::variants::read_file))]
    ReadVariantFile {
        /// Absolute path to the variant file that failed to read.
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// An error that occurs when a task is requested which could not be found.
/// TODO: Make this error better.
///     - Include names that might have been meant instead
///     - If the tasks is only available for a certain platform, explain that.
#[derive(Debug, Clone, Diagnostic, Error)]
#[error("the task '{0}' could not be found", task_name.fancy_display())]
pub struct UnknownTask<'p> {
    /// The project that the platform is not supported for.
    pub project: &'p Workspace,

    /// The environment that the platform is not supported for.
    pub environment: EnvironmentName,

    /// The platform that was requested (if any)
    pub platform: Option<PixiPlatformName>,

    /// The name of the task
    pub task_name: TaskName,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rattler_conda_types::{PackageName, Version};
    use std::str::FromStr;

    fn vp(name: &str, version: &str) -> GenericVirtualPackage {
        GenericVirtualPackage {
            name: PackageName::from_str(name).unwrap(),
            version: Version::from_str(version).unwrap(),
            build_string: String::new(),
        }
    }

    fn err(unsatisfied: Vec<GenericVirtualPackage>) -> UnsupportedPlatformError {
        UnsupportedPlatformError {
            environments_platforms: vec![],
            environment: EnvironmentName::Default,
            platform: Platform::Linux64,
            unsatisfied_requirements: unsatisfied,
        }
    }

    #[test]
    fn missing_cuda_reports_requirement_and_override_hint() {
        let e = err(vec![vp("__cuda", "11")]);
        let display = e.to_string();
        assert!(
            display.contains("Unsatisfied requirements: __cuda >= 11"),
            "{display}"
        );
        let help = e.help().unwrap().to_string();
        assert!(help.contains("CONDA_OVERRIDE_CUDA=11"), "{help}");
    }

    #[test]
    fn missing_multiple_vps_list_all_overrides() {
        let e = err(vec![vp("__cuda", "12.0"), vp("__glibc", "2.27")]);
        let help = e.help().unwrap().to_string();
        assert!(help.contains("CONDA_OVERRIDE_CUDA=12"), "{help}");
        assert!(help.contains("CONDA_OVERRIDE_GLIBC=2.27"), "{help}");
    }

    #[test]
    fn named_environment_renders_unsatisfied_requirements() {
        let mut e = err(vec![vp("__cuda", "11")]);
        e.environment = EnvironmentName::Named("gpu".into());
        let display = e.to_string();
        assert!(
            display.contains("the environment 'gpu' does not support 'linux-64' on this machine"),
            "{display}"
        );
        assert!(
            display.contains("Unsatisfied requirements: __cuda >= 11"),
            "{display}"
        );
    }

    #[test]
    fn empty_unsatisfied_keeps_legacy_supported_platforms_help() {
        let mut e = err(vec![]);
        e.environments_platforms = vec![PixiPlatformName::try_from("linux-64").unwrap()];
        let display = e.to_string();
        assert!(
            display.contains("Add it with 'pixi workspace platform add linux-64'"),
            "{display}"
        );
        let help = e.help().unwrap().to_string();
        assert!(help.contains("supported platforms are linux-64"), "{help}");
    }

    #[test]
    fn unknown_vp_name_skips_override_hint_but_still_lists_requirement() {
        // Version 0 means "must be present at any version" and renders as
        // such instead of a nonsensical ">= 0".
        let e = err(vec![vp("__unix", "0")]);
        let display = e.to_string();
        assert!(
            display.contains("Unsatisfied requirements: __unix (any version)"),
            "{display}"
        );
        let help = e.help().unwrap().to_string();
        assert!(!help.contains("CONDA_OVERRIDE"), "{help}");
    }

    #[test]
    fn versionless_requirement_suggests_realistic_override() {
        let e = err(vec![vp("__cuda", "0")]);
        let display = e.to_string();
        assert!(
            display.contains("Unsatisfied requirements: __cuda (any version)"),
            "{display}"
        );
        let help = e.help().unwrap().to_string();
        assert!(help.contains("CONDA_OVERRIDE_CUDA=12.0"), "{help}");
    }
}
