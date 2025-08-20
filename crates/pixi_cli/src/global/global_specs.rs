use std::path::Path;

use clap::Parser;
use miette::Diagnostic;
use url::Url;

use pixi_consts::consts;
use pixi_global::project::FromMatchSpecError;
use pixi_spec::PixiSpec;
use rattler_conda_types::{ChannelConfig, MatchSpec, ParseMatchSpecError, ParseStrictness};
use typed_path::Utf8NativePathBuf;

use crate::has_specs::HasSpecs;

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
    pub rev: Option<crate::cli_config::GitRev>,

    /// The subdirectory within the git repository
    #[clap(long, requires = "git", help_heading = consts::CLAP_GIT_OPTIONS)]
    pub subdir: Option<String>,

    /// The path to the local directory
    #[clap(long, conflicts_with = "git")]
    pub path: Option<Utf8NativePathBuf>,
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
    #[error("failed to infer package name")]
    #[diagnostic(transparent)]
    PackageNameInference(#[from] pixi_global::project::InferPackageNameError),
}

impl GlobalSpecs {
    /// Convert GlobalSpecs to a vector of GlobalSpec instances
    pub async fn to_global_specs(
        &self,
        channel_config: &ChannelConfig,
        manifest_root: &Path,
        project: &pixi_global::Project,
    ) -> Result<Vec<pixi_global::project::GlobalSpec>, GlobalSpecsConversionError> {
        let git_or_path_spec = if let Some(git_url) = &self.git {
            let git_spec = pixi_spec::GitSpec {
                git: git_url.clone(),
                rev: self.rev.clone().map(Into::into),
                subdirectory: self.subdir.clone(),
            };
            Some(PixiSpec::Git(git_spec))
        } else if let Some(path) = &self.path {
            let absolute_path = dunce::canonicalize(path.as_str())
                .map_err(|_| GlobalSpecsConversionError::AbsolutizePath(path.to_string()))?;

            let relative_path =
                pathdiff::diff_paths(&absolute_path, manifest_root).ok_or_else(|| {
                    GlobalSpecsConversionError::RelativePath(
                        absolute_path.to_string_lossy().to_string(),
                        manifest_root.to_string_lossy().to_string(),
                    )
                })?;
            Some(PixiSpec::Path(pixi_spec::PathSpec {
                path: Utf8NativePathBuf::from(relative_path.to_string_lossy().to_string())
                    .to_typed_path_buf(),
            }))
        } else {
            None
        };
        if let Some(pixi_spec) = git_or_path_spec {
            if self.specs.is_empty() {
                // Infer the package name from the path/git spec
                let inferred_name = project.infer_package_name_from_spec(&pixi_spec).await?;
                return Ok(vec![pixi_global::project::GlobalSpec::new(
                    inferred_name,
                    pixi_spec,
                )]);
            }

            self.specs
                .iter()
                .map(|spec_str| {
                    MatchSpec::from_str(spec_str, ParseStrictness::Lenient)?
                        .name
                        .ok_or(GlobalSpecsConversionError::NameRequired)
                        .map(|name| pixi_global::project::GlobalSpec::new(name, pixi_spec.clone()))
                })
                .collect()
        } else {
            self.specs
                .iter()
                .map(|spec_str| {
                    pixi_global::project::GlobalSpec::try_from_str(spec_str, channel_config)
                        .map_err(Box::new)
                        .map_err(GlobalSpecsConversionError::FromMatchSpec)
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rattler_conda_types::ChannelConfig;

    use super::*;

    #[tokio::test]
    async fn test_to_global_specs_named() {
        let specs = GlobalSpecs {
            specs: vec!["numpy==1.21.0".to_string(), "scipy>=1.7".to_string()],
            git: None,
            rev: None,
            subdir: None,
            path: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");

        // Create a dummy project - this test doesn't use path/git specs so project
        // won't be called
        let project = pixi_global::Project::discover_or_create().await.unwrap();

        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root, &project)
            .await
            .unwrap();

        assert_eq!(global_specs.len(), 2);
    }

    #[tokio::test]
    async fn test_to_global_specs_with_git() {
        let specs = GlobalSpecs {
            specs: vec!["mypackage".to_string()],
            git: Some("https://github.com/user/repo.git".parse().unwrap()),
            rev: None,
            subdir: None,
            path: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");

        // Create a dummy project - this test specifies a package name so inference
        // won't be needed
        let project = pixi_global::Project::discover_or_create().await.unwrap();

        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root, &project)
            .await
            .unwrap();

        assert_eq!(global_specs.len(), 1);
        assert!(matches!(
            &global_specs.first().unwrap().spec,
            &PixiSpec::Git(..)
        ))
    }

    #[tokio::test]
    async fn test_to_global_specs_nameless() {
        let specs = GlobalSpecs {
            specs: vec![">=1.0".to_string()],
            git: None,
            rev: None,
            subdir: None,
            path: None,
        };

        let channel_config = ChannelConfig::default_with_root_dir(PathBuf::from("."));
        let manifest_root = PathBuf::from(".");

        // Create a dummy project
        let project = pixi_global::Project::discover_or_create().await.unwrap();

        let global_specs = specs
            .to_global_specs(&channel_config, &manifest_root, &project)
            .await;
        assert!(global_specs.is_err());
    }
}
