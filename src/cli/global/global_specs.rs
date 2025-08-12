use std::path::Path;

use clap::Parser;
use miette::Diagnostic;
use pixi_consts::consts;
use typed_path::Utf8TypedPathBuf;
use url::Url;

use crate::cli::has_specs::HasSpecs;
use crate::global::project::{FromMatchSpecError, GlobalSpec};
use pixi_spec::PixiSpec;
use rattler_conda_types::{ChannelConfig, MatchSpec, ParseMatchSpecError, ParseStrictness};

#[derive(Parser, Debug, Default, Clone)]
pub struct GlobalSpecs {
    /// The dependency as names, conda MatchSpecs
    #[arg(num_args = 1.., required_unless_present_any = ["git", "path"], value_name = "PACKAGE")]
    pub specs: Vec<String>,

    /// The git url, e.g. `https://github.com/user/repo.git`
    #[clap(long, help_heading = consts::CLAP_GIT_OPTIONS)]
    pub git: Option<Url>,

    /// The git revisions
    #[clap(flatten)]
    pub rev: Option<crate::cli::cli_config::GitRev>,

    /// The subdirectory within the git repository
    #[clap(long, requires = "git", help_heading = consts::CLAP_GIT_OPTIONS)]
    pub subdir: Option<String>,

    /// The path to the local directory
    #[clap(long, conflicts_with = "git")]
    pub path: Option<Utf8TypedPathBuf>,
}

impl HasSpecs for GlobalSpecs {
    fn packages(&self) -> Vec<&str> {
        self.specs.iter().map(AsRef::as_ref).collect()
    }
}

#[derive(Debug, thiserror::Error, Diagnostic)]
pub enum GlobalSpecsConversionError {
    #[error(transparent)]
    ParseMatchSpecError(#[from] ParseMatchSpecError),
    #[error(transparent)]
    FromMatchSpec(#[from] Box<FromMatchSpecError>),
    #[error("package name is required when specifying version constraints without --git or --path")]
    #[diagnostic(
        help = "Use a full package specification like `python==3.12` instead of just `==3.12`"
    )]
    NameRequired,
    #[error("couldn't construct a relative path from {} to {}", .0, .1)]
    RelativePath(String, String),
    #[error("could not absolutize path: {0}")]
    AbsolutizePath(String),
}

impl GlobalSpecs {
    /// Convert GlobalSpecs to a vector of GlobalSpec instances
    pub fn to_global_specs(
        &self,
        channel_config: &ChannelConfig,
        manifest_root: &Path,
    ) -> Result<Vec<GlobalSpec>, GlobalSpecsConversionError> {
        let git_or_path_spec = if let Some(git_url) = &self.git {
            let git_spec = pixi_spec::GitSpec {
                git: git_url.clone(),
                rev: self.rev.clone().map(Into::into),
                subdirectory: self.subdir.clone(),
            };
            Some(PixiSpec::Git(git_spec))
        } else if let Some(path) = &self.path {
            let absolute_path = path
                .absolutize()
                .map_err(|_| GlobalSpecsConversionError::AbsolutizePath(path.to_string()))?;
            let path_str = absolute_path.as_str();
            let relative_path = pathdiff::diff_paths(path_str, manifest_root).ok_or_else(|| {
                GlobalSpecsConversionError::RelativePath(
                    path_str.to_string(),
                    manifest_root.to_string_lossy().to_string(),
                )
            })?;
            Some(PixiSpec::Path(pixi_spec::PathSpec {
                path: Utf8TypedPathBuf::from(relative_path.to_string_lossy().to_string()),
            }))
        } else {
            None
        };
        if let Some(pixi_spec) = git_or_path_spec {
            if self.specs.is_empty() {
                return Ok(Vec::from([GlobalSpec::Nameless(pixi_spec)]));
            }

            self.specs
                .iter()
                .map(|spec_str| {
                    MatchSpec::from_str(spec_str, ParseStrictness::Lenient)?
                        .name
                        .ok_or(GlobalSpecsConversionError::NameRequired)
                        .map(|name| GlobalSpec::named(name, pixi_spec.clone()))
                })
                .collect()
        } else {
            self.specs
                .iter()
                .map(|spec_str| {
                    let global_spec =
                        GlobalSpec::try_from_str(spec_str, channel_config).map_err(Box::new)?;
                    if global_spec.is_nameless() {
                        Err(GlobalSpecsConversionError::NameRequired)
                    } else {
                        Ok(global_spec)
                    }
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use rattler_conda_types::ChannelConfig;

    #[test]
    fn test_to_global_specs_named() {
        let specs = GlobalSpecs {
            specs: vec!["numpy==1.21.0".to_string(), "scipy>=1.7".to_string()],
            git: None,
            rev: None,
            subdir: None,
            path: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");
        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root)
            .unwrap();

        assert_eq!(global_specs.len(), 2);
    }

    #[test]
    fn test_to_global_specs_with_git() {
        let specs = GlobalSpecs {
            specs: vec!["mypackage".to_string()],
            git: Some("https://github.com/user/repo.git".parse().unwrap()),
            rev: None,
            subdir: None,
            path: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");
        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root)
            .unwrap();

        assert_eq!(global_specs.len(), 1);
        assert!(matches!(
            global_specs.first().unwrap().spec(),
            &PixiSpec::Git(..)
        ))
    }

    #[test]
    fn test_to_global_specs_nameless() {
        let specs = GlobalSpecs {
            specs: vec![">=1.0".to_string()],
            git: None,
            rev: None,
            subdir: None,
            path: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");
        let global_specs = specs.to_global_specs(&channel_config, &manifest_root);
        assert!(global_specs.is_err());
    }
}
