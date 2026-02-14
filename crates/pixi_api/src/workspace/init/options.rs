use rattler_conda_types::NamedChannelOrUrl;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{cmp::PartialEq, path::PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct InitOptions {
    /// Where to place the workspace.
    pub path: PathBuf,

    /// Channel to use in the workspace. Defaults to conda-forge when empty.
    pub channels: Option<Vec<NamedChannelOrUrl>>,

    /// Platforms that the workspace supports. Defaults to currently used platform when empty.
    pub platforms: Vec<String>,

    /// Environment.yml file to bootstrap the workspace.
    pub env_file: Option<PathBuf>,

    /// The manifest format to create. Defaults to [ManifestFormat::Pixi] or asks the user when a "pyproject.toml" file already exists.
    pub format: Option<ManifestFormat>,

    /// Source Control Management used for this workspace.
    pub scm: Option<GitAttributes>,

    /// The conda-pypi-mapping
    pub conda_pypi_mapping: Option<HashMap<NamedChannelOrUrl, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ManifestFormat {
    Pixi,
    Pyproject,
    Mojoproject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
pixi.lock merge=binary linguist-language=YAML linguist-generated=true -diff
"#
            }
            GitAttributes::Gitlab => {
                r#"# GitLab syntax highlighting & preventing 3-way merges
pixi.lock merge=binary gitlab-language=yaml gitlab-generated=true -diff
"#
            }
        }
    }
}
