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
    #[arg(num_args = 1.., required = true, value_name = "PACKAGE")]
    pub specs: Vec<String>,

    /// The git url to use when adding a git dependency
    #[clap(hide = true, long, short, help_heading = consts::CLAP_GIT_OPTIONS)]
    pub git: Option<Url>,

    /// The git revisions to use when adding a git dependency
    /// TODO: replace with #[clap(flatten)], skip instead of hide as that doesn't work with flatten
    #[clap(skip)]
    pub rev: Option<crate::cli::cli_config::GitRev>,

    /// The subdirectory of the git repository to use
    #[clap(hide = true, long, short, requires = "git", help_heading = consts::CLAP_GIT_OPTIONS)]
    pub subdir: Option<String>,

    /// The path to the local directory to use when adding a local dependency
    #[clap(hide = true, long, conflicts_with = "git")]
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
    #[error("specs cannot be empty if git or path is not specified")]
    SpecsMissing,
}

impl GlobalSpecs {
    /// Convert GlobalSpecs to a vector of GlobalSpec instances
    pub fn to_global_specs(
        &self,
        channel_config: &ChannelConfig,
    ) -> Result<Vec<GlobalSpec>, GlobalSpecsConversionError> {
        if self.specs.is_empty() && self.git.is_none() && self.path.is_none() {
            return Err(GlobalSpecsConversionError::SpecsMissing);
        }

        let mut result = Vec::with_capacity(self.specs.len());
        for spec_str in &self.specs {
            // Create PixiSpec based on whether we have git/path dependencies
            let pixi_spec = if let Some(git_url) = &self.git {
                // Handle git dependencies
                let git_spec = pixi_spec::GitSpec {
                    git: git_url.clone(),
                    rev: self.rev.clone().map(Into::into),
                    subdirectory: self.subdir.clone(),
                };

                PixiSpec::Git(git_spec)
            } else if let Some(path) = &self.path {
                // Handle path dependencies
                PixiSpec::Path(pixi_spec::PathSpec { path: path.clone() })
            } else {
                // Handle regular conda/version dependencies - use the new try_from_str method
                let global_spec =
                    GlobalSpec::try_from_str(spec_str, channel_config).map_err(Box::new)?;
                if global_spec.is_nameless() {
                    return Err(GlobalSpecsConversionError::NameRequired);
                }
                result.push(global_spec);
                continue;
            };

            // For git/path dependencies, we need to parse the spec to get the name
            let match_spec = MatchSpec::from_str(spec_str, ParseStrictness::Lenient)?;
            if let Some(name) = match_spec.name {
                result.push(GlobalSpec::named(name, pixi_spec));
            } else {
                result.push(GlobalSpec::nameless(pixi_spec));
            }
        }

        Ok(result)
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
        let global_specs = specs.to_global_specs(&channel_config).unwrap();

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
        let global_specs = specs.to_global_specs(&channel_config).unwrap();

        assert_eq!(global_specs.len(), 1);
        assert!(matches!(
            global_specs.first().unwrap().spec(),
            &PixiSpec::Git(..)
        ))
    }

    #[test]
    fn test_to_global_specs_empty() {
        let specs = GlobalSpecs::default();
        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let global_specs = specs.to_global_specs(&channel_config);
        assert!(global_specs.is_err());
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
        let global_specs = specs.to_global_specs(&channel_config);
        assert!(global_specs.is_err());
    }

    #[test]
    fn test_to_global_specs_with_path() {
        let specs = GlobalSpecs {
            specs: vec!["mypackage".to_string()],
            path: Some(Utf8TypedPathBuf::from("../local_package")),
            git: None,
            rev: None,
            subdir: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let global_specs = specs.to_global_specs(&channel_config).unwrap();

        assert_eq!(global_specs.len(), 1);
        assert!(matches!(
            global_specs.first().unwrap().spec(),
            &PixiSpec::Path(..)
        ))
    }
}
