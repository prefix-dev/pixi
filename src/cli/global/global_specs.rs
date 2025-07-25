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
    #[clap(flatten)]
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
            } else if spec_str.ends_with(".conda")
                || spec_str.ends_with(".tar.bz2")
                || spec_str.starts_with("./")
                || spec_str.starts_with("../")
                || spec_str.starts_with('/')
            {
                // Auto-detect .conda/.tar.bz2 files or path-like specs and handle as path dependencies
                PixiSpec::Path(pixi_spec::PathSpec {
                    path: Utf8TypedPathBuf::from(spec_str.to_string()),
                })
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
            // For auto-detected paths, extract the name from the filename if it's a .conda or .tar.bz2 file
            if matches!(pixi_spec, PixiSpec::Path(_))
                && (spec_str.ends_with(".conda")
                    || spec_str.ends_with(".tar.bz2")
                    || spec_str.starts_with("./")
                    || spec_str.starts_with("../")
                    || spec_str.starts_with('/'))
            {
                // For .conda or .tar.bz2 files, extract package name from filename
                if spec_str.ends_with(".conda") || spec_str.ends_with(".tar.bz2") {
                    let filename = std::path::Path::new(spec_str)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or(spec_str);

                    // Extract package name from conda filename (e.g., "curl-8.14.1-h332b0f4_0.conda" or "curl-8.14.1-h332b0f4_0.tar.bz2" -> "curl")
                    if let Some(package_name_part) = filename.split('-').next() {
                        if let Ok(package_name) = rattler_conda_types::PackageName::try_from(
                            package_name_part.to_string(),
                        ) {
                            result.push(GlobalSpec::named(package_name, pixi_spec));
                        } else {
                            result.push(GlobalSpec::nameless(pixi_spec));
                        }
                    } else {
                        result.push(GlobalSpec::nameless(pixi_spec));
                    }
                } else {
                    // For other paths, create nameless spec
                    result.push(GlobalSpec::nameless(pixi_spec));
                }
            } else {
                // For explicit git/path dependencies, parse the spec to get the name
                let match_spec = MatchSpec::from_str(spec_str, ParseStrictness::Lenient)?;
                if let Some(name) = match_spec.name {
                    result.push(GlobalSpec::named(name, pixi_spec));
                } else {
                    result.push(GlobalSpec::nameless(pixi_spec));
                }
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

    #[test]
    fn test_to_global_specs_auto_detect_conda_file() {
        let specs = GlobalSpecs {
            specs: vec!["./curl-8.14.1-h332b0f4_0.conda".to_string()],
            path: None,
            git: None,
            rev: None,
            subdir: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let global_specs = specs.to_global_specs(&channel_config).unwrap();

        assert_eq!(global_specs.len(), 1);
        let global_spec = global_specs.first().unwrap();
        assert!(matches!(global_spec.spec(), &PixiSpec::Path(..)));
        // Should extract "curl" as the package name from "curl-8.14.1-h332b0f4_0.conda"
        match global_spec {
            GlobalSpec::Named(named_spec) => {
                assert_eq!(named_spec.name().as_source(), "curl");
            }
            GlobalSpec::Nameless(_) => panic!("Expected named spec for .conda file"),
        }
    }

    #[test]
    fn test_to_global_specs_auto_detect_relative_path() {
        let specs = GlobalSpecs {
            specs: vec!["../some-package".to_string()],
            path: None,
            git: None,
            rev: None,
            subdir: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let global_specs = specs.to_global_specs(&channel_config).unwrap();

        assert_eq!(global_specs.len(), 1);
        let global_spec = global_specs.first().unwrap();
        assert!(matches!(global_spec.spec(), &PixiSpec::Path(..)));
        // Should be nameless for non-.conda paths
        assert!(matches!(global_spec, GlobalSpec::Nameless(_)));
    }

    #[test]
    fn test_to_global_specs_auto_detect_tar_bz2_file() {
        let specs = GlobalSpecs {
            specs: vec!["./python-3.8.5-h7579374_1.tar.bz2".to_string()],
            path: None,
            git: None,
            rev: None,
            subdir: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let global_specs = specs.to_global_specs(&channel_config).unwrap();

        assert_eq!(global_specs.len(), 1);
        let global_spec = global_specs.first().unwrap();
        assert!(matches!(global_spec.spec(), &PixiSpec::Path(..)));
        // Should extract "python" as the package name from "python-3.8.5-h7579374_1.tar.bz2"
        match global_spec {
            GlobalSpec::Named(named_spec) => {
                assert_eq!(named_spec.name().as_source(), "python");
            }
            GlobalSpec::Nameless(_) => panic!("Expected named spec for .tar.bz2 file"),
        }
    }

    #[test]
    fn test_to_global_specs_auto_detect_absolute_path_tar_bz2() {
        let specs = GlobalSpecs {
            specs: vec!["/tmp/numpy-1.21.0-py38h9894fe3_0.tar.bz2".to_string()],
            path: None,
            git: None,
            rev: None,
            subdir: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let global_specs = specs.to_global_specs(&channel_config).unwrap();

        assert_eq!(global_specs.len(), 1);
        let global_spec = global_specs.first().unwrap();
        assert!(matches!(global_spec.spec(), &PixiSpec::Path(..)));
        // Should extract "numpy" as the package name
        match global_spec {
            GlobalSpec::Named(named_spec) => {
                assert_eq!(named_spec.name().as_source(), "numpy");
            }
            GlobalSpec::Nameless(_) => {
                panic!("Expected named spec for absolute path .tar.bz2 file")
            }
        }
    fn test_parse_from_command_args() {
        // Test parsing simple package name
        let args = vec!["foo", "numpy"];
        let specs = GlobalSpecs::try_parse_from(args).unwrap();
        assert_eq!(specs.specs, vec!["numpy"]);
        assert!(specs.git.is_none());
        assert!(specs.path.is_none());

        // Test parsing multiple packages
        let args = vec!["foo", "numpy", "scipy>=1.7", "matplotlib==3.5.0"];
        let specs = GlobalSpecs::try_parse_from(args).unwrap();
        assert_eq!(
            specs.specs,
            vec!["numpy", "scipy>=1.7", "matplotlib==3.5.0"]
        );

        // Test parsing with git option
        let args = vec![
            "foo",
            "--git",
            "https://github.com/user/repo.git",
            "mypackage",
        ];
        let specs = GlobalSpecs::try_parse_from(args).unwrap();
        assert_eq!(specs.specs, vec!["mypackage"]);
        assert_eq!(
            specs.git.unwrap().as_str(),
            "https://github.com/user/repo.git"
        );

        // Test parsing with path option
        let args = vec!["foo", "--path", "../local_package", "mypackage"];
        let specs = GlobalSpecs::try_parse_from(args).unwrap();
        assert_eq!(specs.specs, vec!["mypackage"]);
        assert_eq!(specs.path.unwrap().as_str(), "../local_package");

        // Test error when no packages specified
        let args = vec!["foo"];
        let result = GlobalSpecs::try_parse_from(args);
        assert!(result.is_err());
    }
}
