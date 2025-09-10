use rattler_conda_types::NamedChannelOrUrl;
use std::{cmp::PartialEq, path::PathBuf};

#[derive(Debug)]
pub struct InitOptions {
    /// Where to place the workspace
    pub path: PathBuf,

    /// Channel to use in the workspace
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Platforms that the workspace supports
    pub platforms: Vec<String>,

    /// Environment.yml file to bootstrap the workspace
    pub env_file: Option<PathBuf>,

    /// The manifest format to create
    pub format: Option<ManifestFormat>,

    /// Source Control Management used for this workspace
    pub scm: Option<GitAttributes>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ManifestFormat {
    Pixi,
    Pyproject,
    Mojoproject,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GitAttributes {
    Github,
    Gitlab,
    Codeberg,
}

impl GitAttributes {
    pub(crate) fn template(&self) -> &'static str {
        match self {
            GitAttributes::Github | GitAttributes::Codeberg => {
                r#"# SCM syntax highlighting & preventing 3-way merges
pixi.lock merge=binary linguist-language=YAML linguist-generated=true
"#
            }
            GitAttributes::Gitlab => {
                r#"# GitLab syntax highlighting & preventing 3-way merges
pixi.lock merge=binary gitlab-language=yaml gitlab-generated=true
"#
            }
        }
    }
}
